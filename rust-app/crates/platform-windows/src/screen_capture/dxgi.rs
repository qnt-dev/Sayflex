use std::{
    fmt, slice,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{E_ACCESSDENIED, HMODULE},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Texture2D,
            },
            Dxgi::{
                Common::{
                    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_IDENTITY,
                    DXGI_MODE_ROTATION_ROTATE90, DXGI_MODE_ROTATION_ROTATE180,
                    DXGI_MODE_ROTATION_ROTATE270, DXGI_SAMPLE_DESC,
                },
                CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_CURRENTLY_AVAILABLE,
                DXGI_ERROR_NOT_FOUND, DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1,
                IDXGIOutputDuplication, IDXGIResource,
            },
        },
        UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
    },
    core::{Error as WindowsError, Interface},
};

use rust_protocol::PhysicalRect;

use super::{
    CaptureError, CapturedOutput, DesktopCapture, DesktopCaptureBackend, Rotation,
    next_capture_generation,
};

const FRAME_TIMEOUT_MILLIS: u32 = 250;
const CACHED_FRAME_WAIT_MILLIS: u32 = 16;
const BOOTSTRAP_FRAME_DEADLINE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapFrameDisposition {
    Use,
    Retry,
    TimedOut,
}

fn classify_bootstrap_frame(
    last_present_time: i64,
    elapsed: Duration,
) -> BootstrapFrameDisposition {
    if last_present_time != 0 {
        BootstrapFrameDisposition::Use
    } else if elapsed < BOOTSTRAP_FRAME_DEADLINE {
        BootstrapFrameDisposition::Retry
    } else {
        BootstrapFrameDisposition::TimedOut
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaptureTopologyEntry {
    bounds: PhysicalRect,
    dpi_scale_milli: u32,
}

fn topology_requires_rebuild(
    cached: &[CaptureTopologyEntry],
    current: &[CaptureTopologyEntry],
) -> bool {
    cached != current
}

pub struct DxgiDesktopCaptureBackend {
    session: Mutex<Option<DxgiCaptureSession>>,
}

impl fmt::Debug for DxgiDesktopCaptureBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DxgiDesktopCaptureBackend")
            .finish_non_exhaustive()
    }
}

impl Default for DxgiDesktopCaptureBackend {
    fn default() -> Self {
        Self {
            session: Mutex::new(None),
        }
    }
}

impl DesktopCaptureBackend for DxgiDesktopCaptureBackend {
    fn capture(&self) -> Result<DesktopCapture, CaptureError> {
        let topology = current_topology()?;
        let mut session = lock(&self.session);
        if session
            .as_ref()
            .is_none_or(|session| topology_requires_rebuild(&session.topology, &topology))
        {
            *session = Some(DxgiCaptureSession::create()?);
        }
        let result = session
            .as_mut()
            .expect("DXGI session was just created")
            .capture();
        if !matches!(result, Err(CaptureError::AccessLost)) {
            return result;
        }

        // Desktop Duplication objects are invalid after a mode switch, UAC
        // transition, driver reset, or output reconfiguration. Rebuild once in
        // place so the next request does not inherit the lost session.
        *session = Some(DxgiCaptureSession::create()?);
        session
            .as_mut()
            .expect("DXGI session was just recreated")
            .capture()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn current_topology() -> Result<Vec<CaptureTopologyEntry>, CaptureError> {
    let mut topology = crate::region_selection::attached_monitor_regions()
        .map_err(CaptureError::BackendFailure)?
        .into_iter()
        .map(|region| CaptureTopologyEntry {
            bounds: region.bounds,
            dpi_scale_milli: region.dpi_scale_milli,
        })
        .collect::<Vec<_>>();
    topology.sort_by_key(|entry| {
        (
            entry.bounds.x,
            entry.bounds.y,
            entry.bounds.width,
            entry.bounds.height,
            entry.dpi_scale_milli,
        )
    });
    Ok(topology)
}

struct DxgiCaptureSession {
    topology: Vec<CaptureTopologyEntry>,
    outputs: Vec<DxgiOutputCapture>,
}

impl DxgiCaptureSession {
    fn create() -> Result<Self, CaptureError> {
        // SAFETY: DXGI returns reference-counted COM interfaces. Each is owned by
        // its Rust wrapper and released automatically after the copied CPU frame is
        // detached into Vec<u8> storage.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(map_dxgi_error)?;
        let mut outputs = Vec::new();
        let mut adapter_index = 0;
        loop {
            let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(map_dxgi_error(error)),
            };
            append_adapter_outputs(&adapter, &mut outputs)?;
            adapter_index += 1;
        }
        if outputs.is_empty() {
            return Err(CaptureError::NoOutputs);
        }
        outputs.sort_by_key(|output| {
            (
                output.bounds.x,
                output.bounds.y,
                output.bounds.width,
                output.bounds.height,
            )
        });
        let topology = outputs
            .iter()
            .map(|output| CaptureTopologyEntry {
                bounds: output.bounds,
                dpi_scale_milli: output.dpi_scale_milli,
            })
            .collect();
        Ok(Self { topology, outputs })
    }

    fn capture(&mut self) -> Result<DesktopCapture, CaptureError> {
        let mut captures = Vec::with_capacity(self.outputs.len());
        for output in &mut self.outputs {
            let captured = capture_output(
                &output.device,
                &output.context,
                &output.duplication,
                output.bounds,
                output.dpi_scale_milli,
                output.last_frame.as_ref(),
            )?;
            output.last_frame = Some(captured.clone());
            captures.push(captured);
        }
        DesktopCapture::try_new(next_capture_generation(), captures)
    }
}

struct DxgiOutputCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    bounds: PhysicalRect,
    dpi_scale_milli: u32,
    last_frame: Option<CapturedOutput>,
}

fn append_adapter_outputs(
    adapter: &IDXGIAdapter1,
    captures: &mut Vec<DxgiOutputCapture>,
) -> Result<(), CaptureError> {
    let (device, context) = create_device(adapter)?;
    let mut output_index = 0;
    loop {
        let output = match unsafe { adapter.EnumOutputs(output_index) } {
            Ok(output) => output,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(map_dxgi_error(error)),
        };
        output_index += 1;
        let description = unsafe { output.GetDesc() }.map_err(map_dxgi_error)?;
        if !description.AttachedToDesktop.as_bool() {
            continue;
        }
        let output: IDXGIOutput1 = output.cast().map_err(map_dxgi_error)?;
        let duplication = unsafe { output.DuplicateOutput(&device) }.map_err(map_dxgi_error)?;
        let desktop_width = u32::try_from(
            description.DesktopCoordinates.right - description.DesktopCoordinates.left,
        )
        .map_err(|_| CaptureError::InvalidGeometry)?;
        let desktop_height = u32::try_from(
            description.DesktopCoordinates.bottom - description.DesktopCoordinates.top,
        )
        .map_err(|_| CaptureError::InvalidGeometry)?;
        captures.push(DxgiOutputCapture {
            device: device.clone(),
            context: context.clone(),
            duplication,
            bounds: PhysicalRect {
                x: description.DesktopCoordinates.left,
                y: description.DesktopCoordinates.top,
                width: desktop_width,
                height: desktop_height,
            },
            dpi_scale_milli: dpi_scale_milli(description.Monitor),
            last_frame: None,
        });
    }
    Ok(())
}

fn create_device(
    adapter: &IDXGIAdapter1,
) -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(map_dxgi_error)?;
    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(CaptureError::BackendFailure(
            "D3D11CreateDevice returned no device context".into(),
        )),
    }
}

fn capture_output(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    duplication: &IDXGIOutputDuplication,
    bounds: PhysicalRect,
    dpi_scale_milli: u32,
    last_frame: Option<&CapturedOutput>,
) -> Result<CapturedOutput, CaptureError> {
    let bootstrap_started = Instant::now();
    let (frame_info, desktop_resource, mut lease) = loop {
        let elapsed = bootstrap_started.elapsed();
        let remaining = BOOTSTRAP_FRAME_DEADLINE.saturating_sub(elapsed);
        let timeout_millis = u32::try_from(remaining.as_millis())
            .unwrap_or(FRAME_TIMEOUT_MILLIS)
            .clamp(1, FRAME_TIMEOUT_MILLIS);
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut desktop_resource: Option<IDXGIResource> = None;
        let acquire_result = unsafe {
            duplication.AcquireNextFrame(
                if last_frame.is_some() {
                    CACHED_FRAME_WAIT_MILLIS
                } else {
                    timeout_millis
                },
                &mut frame_info,
                &mut desktop_resource,
            )
        };
        if let Err(error) = acquire_result {
            if error.code() == DXGI_ERROR_WAIT_TIMEOUT
                && let Some(last_frame) = last_frame
            {
                return Ok(last_frame.clone());
            }
            return Err(map_dxgi_error(error));
        }
        let mut lease = AcquiredFrame::new(duplication);
        if frame_info.LastPresentTime == 0
            && let Some(last_frame) = last_frame
        {
            lease.release()?;
            return Ok(last_frame.clone());
        }
        match classify_bootstrap_frame(frame_info.LastPresentTime, bootstrap_started.elapsed()) {
            BootstrapFrameDisposition::Use => {
                break (frame_info, desktop_resource, lease);
            }
            BootstrapFrameDisposition::Retry => {
                lease.release()?;
            }
            BootstrapFrameDisposition::TimedOut => {
                lease.release()?;
                return Err(CaptureError::TemporaryUnavailable(
                    "DXGI did not produce a presented desktop frame within 100 ms".into(),
                ));
            }
        }
    };
    let protected_content_masked = frame_info.ProtectedContentMaskedOut.as_bool();
    let desktop_resource = desktop_resource.ok_or_else(|| {
        CaptureError::BackendFailure("DXGI returned a frame without a resource".into())
    })?;
    let texture: ID3D11Texture2D = desktop_resource.cast().map_err(map_dxgi_error)?;
    let mut texture_desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut texture_desc) };
    if texture_desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
        return Err(CaptureError::BackendFailure(format!(
            "DXGI returned unsupported desktop format {:?}",
            texture_desc.Format
        )));
    }
    let duplication_desc = unsafe { duplication.GetDesc() };
    let rotation = map_rotation(duplication_desc.Rotation)?;
    let expected_source_dimensions = rotation.source_dimensions(bounds.width, bounds.height);
    if (texture_desc.Width, texture_desc.Height) != expected_source_dimensions {
        return Err(CaptureError::BackendFailure(format!(
            "DXGI frame dimensions {}x{} do not match output {}x{} with {rotation:?}",
            texture_desc.Width, texture_desc.Height, bounds.width, bounds.height
        )));
    }

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: texture_desc.Width,
        Height: texture_desc.Height,
        MipLevels: 1,
        ArraySize: 1,
        Format: texture_desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }
        .map_err(map_dxgi_error)?;
    let staging = staging.ok_or_else(|| {
        CaptureError::BackendFailure("D3D11 staging texture was not created".into())
    })?;
    unsafe { context.CopyResource(&staging, &texture) };

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
        .map_err(map_dxgi_error)?;
    let mapped_guard = MappedTexture::new(context, &staging);
    if mapped.pData.is_null() {
        return Err(CaptureError::BackendFailure(
            "D3D11 mapped the staging texture to null".into(),
        ));
    }
    let tight_stride = texture_desc
        .Width
        .checked_mul(4)
        .ok_or(CaptureError::StrideOverflow {
            width: texture_desc.Width,
        })?;
    if mapped.RowPitch < tight_stride {
        return Err(CaptureError::StrideTooSmall {
            minimum: tight_stride,
            actual: mapped.RowPitch,
        });
    }
    let byte_len = usize::try_from(
        u64::from(tight_stride)
            .checked_mul(u64::from(texture_desc.Height))
            .ok_or(CaptureError::InvalidGeometry)?,
    )
    .map_err(|_| CaptureError::InvalidGeometry)?;
    if byte_len > super::MAX_CAPTURE_BYTES {
        return Err(CaptureError::AllocationTooLarge {
            requested: byte_len as u64,
            limit: super::MAX_CAPTURE_BYTES,
        });
    }
    let source_len = usize::try_from(
        u64::from(mapped.RowPitch)
            .checked_mul(u64::from(texture_desc.Height))
            .ok_or(CaptureError::InvalidGeometry)?,
    )
    .map_err(|_| CaptureError::InvalidGeometry)?;
    let bgra = unsafe { slice::from_raw_parts(mapped.pData.cast::<u8>(), source_len) };
    let mut rgba = vec![0_u8; byte_len];
    for row in 0..texture_desc.Height as usize {
        let source_row = &bgra[row * mapped.RowPitch as usize
            ..row * mapped.RowPitch as usize + tight_stride as usize];
        let target_row = &mut rgba[row * tight_stride as usize..(row + 1) * tight_stride as usize];
        for (source, target) in source_row
            .chunks_exact(4)
            .zip(target_row.chunks_exact_mut(4))
        {
            target.copy_from_slice(&[source[2], source[1], source[0], u8::MAX]);
        }
    }
    drop(mapped_guard);
    lease.release()?;

    CapturedOutput::try_new_with_protection(
        bounds,
        rotation,
        dpi_scale_milli,
        tight_stride,
        rgba,
        protected_content_masked,
    )
}

fn map_rotation(rotation: DXGI_MODE_ROTATION) -> Result<Rotation, CaptureError> {
    if rotation == DXGI_MODE_ROTATION_IDENTITY {
        Ok(Rotation::Identity)
    } else if rotation == DXGI_MODE_ROTATION_ROTATE90 {
        Ok(Rotation::Rotate90)
    } else if rotation == DXGI_MODE_ROTATION_ROTATE180 {
        Ok(Rotation::Rotate180)
    } else if rotation == DXGI_MODE_ROTATION_ROTATE270 {
        Ok(Rotation::Rotate270)
    } else {
        Err(CaptureError::UnsupportedRotation)
    }
}

fn dpi_scale_milli(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> u32 {
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err() {
        return 1_000;
    }
    dpi_x.saturating_mul(1_000) / 96
}

fn map_dxgi_error(error: WindowsError) -> CaptureError {
    let code = error.code();
    if code == DXGI_ERROR_ACCESS_LOST {
        CaptureError::AccessLost
    } else if code == E_ACCESSDENIED {
        CaptureError::SecureDesktop
    } else if code == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE
        || code == DXGI_ERROR_WAIT_TIMEOUT
        || code == DXGI_ERROR_UNSUPPORTED
    {
        CaptureError::TemporaryUnavailable(format!("0x{:08X}", code.0 as u32))
    } else {
        CaptureError::BackendFailure(format!("{error} (0x{:08X})", code.0 as u32))
    }
}

struct AcquiredFrame<'a> {
    duplication: &'a IDXGIOutputDuplication,
    active: bool,
}

impl<'a> AcquiredFrame<'a> {
    fn new(duplication: &'a IDXGIOutputDuplication) -> Self {
        Self {
            duplication,
            active: true,
        }
    }

    fn release(&mut self) -> Result<(), CaptureError> {
        if self.active {
            unsafe { self.duplication.ReleaseFrame() }.map_err(map_dxgi_error)?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for AcquiredFrame<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = unsafe { self.duplication.ReleaseFrame() };
        }
    }
}

struct MappedTexture<'a> {
    context: &'a ID3D11DeviceContext,
    texture: &'a ID3D11Texture2D,
}

impl<'a> MappedTexture<'a> {
    fn new(context: &'a ID3D11DeviceContext, texture: &'a ID3D11Texture2D) -> Self {
        Self { context, texture }
    }
}

impl Drop for MappedTexture<'_> {
    fn drop(&mut self) {
        unsafe { self.context.Unmap(self.texture, 0) };
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rust_protocol::PhysicalRect;

    use super::{
        BootstrapFrameDisposition, CaptureTopologyEntry, classify_bootstrap_frame,
        topology_requires_rebuild,
    };

    #[test]
    fn a_fresh_duplication_retries_the_zero_timestamp_frame_but_accepts_the_next_present() {
        assert_eq!(
            classify_bootstrap_frame(0, Duration::from_millis(5)),
            BootstrapFrameDisposition::Retry
        );
        assert_eq!(
            classify_bootstrap_frame(3_463_825_522_915, Duration::from_millis(10)),
            BootstrapFrameDisposition::Use
        );
    }

    #[test]
    fn a_driver_that_never_presents_is_bounded_instead_of_spinning_forever() {
        assert_eq!(
            classify_bootstrap_frame(0, Duration::from_millis(100)),
            BootstrapFrameDisposition::TimedOut
        );
    }

    #[test]
    fn cached_capture_resources_survive_unchanged_desktops_but_not_layout_or_dpi_changes() {
        let original = [CaptureTopologyEntry {
            bounds: PhysicalRect {
                x: 0,
                y: 0,
                width: 1_920,
                height: 1_080,
            },
            dpi_scale_milli: 1_000,
        }];
        assert!(!topology_requires_rebuild(&original, &original));

        let scaled = [CaptureTopologyEntry {
            dpi_scale_milli: 1_250,
            ..original[0]
        }];
        assert!(topology_requires_rebuild(&original, &scaled));

        let moved = [CaptureTopologyEntry {
            bounds: PhysicalRect {
                x: -1_920,
                ..original[0].bounds
            },
            ..original[0]
        }];
        assert!(topology_requires_rebuild(&original, &moved));
    }
}
