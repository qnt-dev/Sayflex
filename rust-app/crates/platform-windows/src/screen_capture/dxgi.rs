use std::slice;

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

#[derive(Debug, Default)]
pub struct DxgiDesktopCaptureBackend;

impl DesktopCaptureBackend for DxgiDesktopCaptureBackend {
    fn capture(&self) -> Result<DesktopCapture, CaptureError> {
        capture_attached_outputs()
    }
}

fn capture_attached_outputs() -> Result<DesktopCapture, CaptureError> {
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
        capture_adapter_outputs(&adapter, &mut outputs)?;
        adapter_index += 1;
    }
    DesktopCapture::try_new(next_capture_generation(), outputs)
}

fn capture_adapter_outputs(
    adapter: &IDXGIAdapter1,
    captures: &mut Vec<CapturedOutput>,
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
        captures.push(capture_output(
            &device,
            &context,
            &duplication,
            description.DesktopCoordinates,
            description.Monitor,
        )?);
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
    desktop_coordinates: windows::Win32::Foundation::RECT,
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
) -> Result<CapturedOutput, CaptureError> {
    let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut desktop_resource: Option<IDXGIResource> = None;
    unsafe {
        duplication.AcquireNextFrame(FRAME_TIMEOUT_MILLIS, &mut frame_info, &mut desktop_resource)
    }
    .map_err(map_dxgi_error)?;
    let mut lease = AcquiredFrame::new(duplication);
    if frame_info.ProtectedContentMaskedOut.as_bool() {
        return Err(CaptureError::ProtectedContent);
    }
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
    let desktop_width = u32::try_from(desktop_coordinates.right - desktop_coordinates.left)
        .map_err(|_| CaptureError::InvalidGeometry)?;
    let desktop_height = u32::try_from(desktop_coordinates.bottom - desktop_coordinates.top)
        .map_err(|_| CaptureError::InvalidGeometry)?;
    let bounds = PhysicalRect {
        x: desktop_coordinates.left,
        y: desktop_coordinates.top,
        width: desktop_width,
        height: desktop_height,
    };
    let expected_source_dimensions = rotation.source_dimensions(desktop_width, desktop_height);
    if (texture_desc.Width, texture_desc.Height) != expected_source_dimensions {
        return Err(CaptureError::BackendFailure(format!(
            "DXGI frame dimensions {}x{} do not match output {}x{} with {rotation:?}",
            texture_desc.Width, texture_desc.Height, desktop_width, desktop_height
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

    CapturedOutput::try_new(
        bounds,
        rotation,
        dpi_scale_milli(monitor),
        tight_stride,
        rgba,
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
