use rust_protocol::PhysicalRect;
use thiserror::Error;

#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
mod gdi;

const BYTES_PER_PIXEL: u32 = 4;
pub const MAX_CAPTURE_BYTES: usize = 512 * 1024 * 1024;

#[cfg(windows)]
static CAPTURE_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CaptureError {
    #[error("the capture region has no area")]
    EmptyRegion,
    #[error("the capture region is outside the virtual desktop")]
    OutsideDesktop,
    #[error("capture geometry exceeds the supported coordinate range")]
    InvalidGeometry,
    #[error("RGBA stride {actual} is smaller than the required {minimum} bytes")]
    StrideTooSmall { minimum: u32, actual: u32 },
    #[error("RGBA stride overflows for a {width}-pixel-wide frame")]
    StrideOverflow { width: u32 },
    #[error("RGBA buffer length is {actual} bytes, expected {expected}")]
    InvalidBufferLength { expected: usize, actual: usize },
    #[error("capture would allocate {requested} bytes, above the {limit}-byte safety limit")]
    AllocationTooLarge { requested: u64, limit: usize },
    #[error("captured outputs {first} and {second} overlap")]
    OverlappingOutputs { first: usize, second: usize },
    #[error("no attached desktop outputs were found")]
    NoOutputs,
    #[error("the desktop capture backend is temporarily unavailable: {0}")]
    TemporaryUnavailable(String),
    #[error("desktop duplication access was lost")]
    AccessLost,
    #[error("protected content was masked in the captured frame")]
    ProtectedContent,
    #[error("the secure desktop cannot be captured")]
    SecureDesktop,
    #[error("the output reports an unsupported rotation")]
    UnsupportedRotation,
    #[error("desktop capture failed: {0}")]
    BackendFailure(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    #[default]
    Identity,
    Rotate90,
    Rotate180,
    Rotate270,
}

impl Rotation {
    #[must_use]
    pub const fn source_dimensions(self, desktop_width: u32, desktop_height: u32) -> (u32, u32) {
        match self {
            Self::Identity | Self::Rotate180 => (desktop_width, desktop_height),
            Self::Rotate90 | Self::Rotate270 => (desktop_height, desktop_width),
        }
    }

    fn source_point(
        self,
        desktop_x: u32,
        desktop_y: u32,
        source_width: u32,
        source_height: u32,
    ) -> (u32, u32) {
        match self {
            Self::Identity => (desktop_x, desktop_y),
            Self::Rotate90 => (desktop_y, source_height - 1 - desktop_x),
            Self::Rotate180 => (source_width - 1 - desktop_x, source_height - 1 - desktop_y),
            Self::Rotate270 => (source_width - 1 - desktop_y, desktop_x),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaFrame {
    pub rect: PhysicalRect,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

impl RgbaFrame {
    pub fn try_new(rect: PhysicalRect, stride: u32, pixels: Vec<u8>) -> Result<Self, CaptureError> {
        validate_rect(rect)?;
        let minimum = rect
            .width
            .checked_mul(BYTES_PER_PIXEL)
            .ok_or(CaptureError::StrideOverflow { width: rect.width })?;
        if stride < minimum {
            return Err(CaptureError::StrideTooSmall {
                minimum,
                actual: stride,
            });
        }
        let expected = checked_byte_len(stride, rect.height)?;
        if pixels.len() != expected {
            return Err(CaptureError::InvalidBufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            rect,
            stride,
            pixels,
        })
    }

    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.rect.width, self.rect.height)
    }

    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.rect.width || y >= self.rect.height {
            return None;
        }
        let offset = y as usize * self.stride as usize + x as usize * BYTES_PER_PIXEL as usize;
        self.pixels
            .get(offset..offset + BYTES_PER_PIXEL as usize)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedOutput {
    pub bounds: PhysicalRect,
    pub rotation: Rotation,
    pub dpi_scale_milli: u32,
    frame: RgbaFrame,
}

impl CapturedOutput {
    pub fn try_new(
        bounds: PhysicalRect,
        rotation: Rotation,
        dpi_scale_milli: u32,
        stride: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, CaptureError> {
        validate_rect(bounds)?;
        let (source_width, source_height) = rotation.source_dimensions(bounds.width, bounds.height);
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: source_width,
                height: source_height,
            },
            stride,
            pixels,
        )?;
        Ok(Self {
            bounds,
            rotation,
            dpi_scale_milli,
            frame,
        })
    }

    fn rgba_at_desktop(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let (source_x, source_y) =
            self.rotation
                .source_point(x, y, self.frame.rect.width, self.frame.rect.height);
        self.frame.pixel(source_x, source_y)
    }

    fn owned_frame_bytes(&self) -> usize {
        self.frame.pixels.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopCapture {
    generation: u64,
    outputs: Vec<CapturedOutput>,
    bounds: PhysicalRect,
}

impl DesktopCapture {
    pub fn try_new(generation: u64, outputs: Vec<CapturedOutput>) -> Result<Self, CaptureError> {
        if outputs.is_empty() {
            return Err(CaptureError::NoOutputs);
        }
        for (first_index, first) in outputs.iter().enumerate() {
            for (second_index, second) in outputs.iter().enumerate().skip(first_index + 1) {
                if intersection(first.bounds, second.bounds)?.is_some() {
                    return Err(CaptureError::OverlappingOutputs {
                        first: first_index,
                        second: second_index,
                    });
                }
            }
        }
        let total_bytes = outputs.iter().try_fold(0_u64, |total, output| {
            total
                .checked_add(output.owned_frame_bytes() as u64)
                .ok_or(CaptureError::InvalidGeometry)
        })?;
        ensure_within_byte_cap(total_bytes)?;
        let bounds = bounding_rect(outputs.iter().map(|output| output.bounds))?;
        Ok(Self {
            generation,
            outputs,
            bounds,
        })
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn bounds(&self) -> PhysicalRect {
        self.bounds
    }

    #[must_use]
    pub fn owned_frame_bytes(&self) -> usize {
        self.outputs
            .iter()
            .map(CapturedOutput::owned_frame_bytes)
            .sum()
    }

    pub fn crop(&self, requested: PhysicalRect) -> Result<RgbaFrame, CaptureError> {
        validate_rect(requested)?;
        let clipped = intersection(requested, self.bounds)?.ok_or(CaptureError::OutsideDesktop)?;
        let stride =
            clipped
                .width
                .checked_mul(BYTES_PER_PIXEL)
                .ok_or(CaptureError::StrideOverflow {
                    width: clipped.width,
                })?;
        let byte_len = checked_byte_len(stride, clipped.height)?;
        let mut pixels = vec![0; byte_len];
        for alpha in pixels.iter_mut().skip(3).step_by(4) {
            *alpha = u8::MAX;
        }

        for output in &self.outputs {
            let Some(copy_rect) = intersection(clipped, output.bounds)? else {
                continue;
            };
            for desktop_y in copy_rect.y..rect_bottom(copy_rect)? {
                for desktop_x in copy_rect.x..rect_right(copy_rect)? {
                    let output_x = (i64::from(desktop_x) - i64::from(output.bounds.x)) as u32;
                    let output_y = (i64::from(desktop_y) - i64::from(output.bounds.y)) as u32;
                    let target_x = (i64::from(desktop_x) - i64::from(clipped.x)) as usize;
                    let target_y = (i64::from(desktop_y) - i64::from(clipped.y)) as usize;
                    let offset = target_y * stride as usize + target_x * BYTES_PER_PIXEL as usize;
                    if let Some(rgba) = output.rgba_at_desktop(output_x, output_y) {
                        pixels[offset..offset + BYTES_PER_PIXEL as usize].copy_from_slice(&rgba);
                    }
                }
            }
        }
        RgbaFrame::try_new(clipped, stride, pixels)
    }
}

pub trait DesktopCaptureBackend: Send + Sync {
    fn capture(&self) -> Result<DesktopCapture, CaptureError>;
}

pub fn capture_with_fallback(
    primary: &dyn DesktopCaptureBackend,
    fallback: &dyn DesktopCaptureBackend,
) -> Result<DesktopCapture, CaptureError> {
    match primary.capture() {
        Ok(capture) => Ok(capture),
        Err(CaptureError::TemporaryUnavailable(_)) => fallback.capture(),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
pub use dxgi::DxgiDesktopCaptureBackend;
#[cfg(windows)]
pub use gdi::GdiDesktopCaptureBackend;

#[cfg(windows)]
#[derive(Debug, Default)]
pub struct WindowsDesktopCaptureBackend {
    dxgi: DxgiDesktopCaptureBackend,
    gdi: GdiDesktopCaptureBackend,
}

#[cfg(windows)]
impl DesktopCaptureBackend for WindowsDesktopCaptureBackend {
    fn capture(&self) -> Result<DesktopCapture, CaptureError> {
        capture_with_fallback(&self.dxgi, &self.gdi)
    }
}

fn validate_rect(rect: PhysicalRect) -> Result<(), CaptureError> {
    if rect.width == 0 || rect.height == 0 {
        return Err(CaptureError::EmptyRegion);
    }
    rect_right(rect)?;
    rect_bottom(rect)?;
    Ok(())
}

fn rect_right(rect: PhysicalRect) -> Result<i32, CaptureError> {
    let right = i64::from(rect.x) + i64::from(rect.width);
    i32::try_from(right).map_err(|_| CaptureError::InvalidGeometry)
}

fn rect_bottom(rect: PhysicalRect) -> Result<i32, CaptureError> {
    let bottom = i64::from(rect.y) + i64::from(rect.height);
    i32::try_from(bottom).map_err(|_| CaptureError::InvalidGeometry)
}

fn intersection(
    first: PhysicalRect,
    second: PhysicalRect,
) -> Result<Option<PhysicalRect>, CaptureError> {
    let left = first.x.max(second.x);
    let top = first.y.max(second.y);
    let right = rect_right(first)?.min(rect_right(second)?);
    let bottom = rect_bottom(first)?.min(rect_bottom(second)?);
    if right <= left || bottom <= top {
        return Ok(None);
    }
    Ok(Some(PhysicalRect {
        x: left,
        y: top,
        width: u32::try_from(right - left).map_err(|_| CaptureError::InvalidGeometry)?,
        height: u32::try_from(bottom - top).map_err(|_| CaptureError::InvalidGeometry)?,
    }))
}

fn bounding_rect(
    rects: impl IntoIterator<Item = PhysicalRect>,
) -> Result<PhysicalRect, CaptureError> {
    let mut rects = rects.into_iter();
    let first = rects.next().ok_or(CaptureError::NoOutputs)?;
    let mut left = first.x;
    let mut top = first.y;
    let mut right = rect_right(first)?;
    let mut bottom = rect_bottom(first)?;
    for rect in rects {
        left = left.min(rect.x);
        top = top.min(rect.y);
        right = right.max(rect_right(rect)?);
        bottom = bottom.max(rect_bottom(rect)?);
    }
    Ok(PhysicalRect {
        x: left,
        y: top,
        width: u32::try_from(right - left).map_err(|_| CaptureError::InvalidGeometry)?,
        height: u32::try_from(bottom - top).map_err(|_| CaptureError::InvalidGeometry)?,
    })
}

fn checked_byte_len(stride: u32, height: u32) -> Result<usize, CaptureError> {
    let requested = u64::from(stride)
        .checked_mul(u64::from(height))
        .ok_or(CaptureError::InvalidGeometry)?;
    ensure_within_byte_cap(requested)?;
    usize::try_from(requested).map_err(|_| CaptureError::InvalidGeometry)
}

fn ensure_within_byte_cap(requested: u64) -> Result<(), CaptureError> {
    if requested > MAX_CAPTURE_BYTES as u64 {
        return Err(CaptureError::AllocationTooLarge {
            requested,
            limit: MAX_CAPTURE_BYTES,
        });
    }
    Ok(())
}

#[cfg(windows)]
fn next_capture_generation() -> u64 {
    CAPTURE_GENERATION.fetch_add(1, Ordering::Relaxed)
}
