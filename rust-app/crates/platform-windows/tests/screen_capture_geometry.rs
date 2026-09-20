use std::sync::{Arc, Mutex};

use rust_platform_windows::screen_capture::{
    CaptureError, CapturedOutput, DesktopCapture, DesktopCaptureBackend, RgbaFrame, Rotation,
    capture_with_fallback,
};
use rust_protocol::PhysicalRect;

fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect {
    PhysicalRect {
        x,
        y,
        width,
        height,
    }
}

fn solid_output(
    bounds: PhysicalRect,
    rotation: Rotation,
    dpi_scale_milli: u32,
    rgba: [u8; 4],
) -> CapturedOutput {
    let (source_width, source_height) = rotation.source_dimensions(bounds.width, bounds.height);
    let stride = source_width.checked_mul(4).unwrap();
    let pixels = rgba
        .into_iter()
        .cycle()
        .take(stride as usize * source_height as usize)
        .collect();
    CapturedOutput::try_new(bounds, rotation, dpi_scale_milli, stride, pixels).unwrap()
}

fn coordinate_output(
    bounds: PhysicalRect,
    rotation: Rotation,
    dpi_scale_milli: u32,
) -> CapturedOutput {
    let (source_width, source_height) = rotation.source_dimensions(bounds.width, bounds.height);
    let stride = source_width.checked_mul(4).unwrap();
    let mut pixels = vec![0; stride as usize * source_height as usize];
    for source_y in 0..source_height {
        for source_x in 0..source_width {
            let offset = source_y as usize * stride as usize + source_x as usize * 4;
            pixels[offset..offset + 4].copy_from_slice(&expected_source_pixel(source_x, source_y));
        }
    }
    CapturedOutput::try_new(bounds, rotation, dpi_scale_milli, stride, pixels).unwrap()
}

fn expected_source_pixel(source_x: u32, source_y: u32) -> [u8; 4] {
    [
        source_x as u8,
        source_y as u8,
        (source_x ^ source_y) as u8,
        255,
    ]
}

#[test]
fn crop_composes_negative_origin_and_rotated_outputs_in_physical_pixels() {
    let capture = DesktopCapture::try_new(
        7,
        vec![
            solid_output(
                rect(-4, 0, 4, 6),
                Rotation::Identity,
                1_000,
                [255, 0, 0, 255],
            ),
            coordinate_output(rect(0, 0, 6, 4), Rotation::Rotate90, 1_000),
        ],
    )
    .unwrap();

    let crop = capture.crop(rect(-4, 1, 8, 3)).unwrap();

    assert_eq!(capture.generation(), 7);
    assert_eq!(crop.dimensions(), (8, 3));
    assert_eq!(crop.pixel(0, 0), Some([255, 0, 0, 255]));
    // Desktop-local (3, 3) on the right output maps through a clockwise
    // 90-degree output rotation to source-local (3, 2).
    assert_eq!(crop.pixel(7, 2), Some(expected_source_pixel(3, 2)));
}

#[test]
fn crop_clamps_to_the_virtual_desktop_without_rescaling_pixels() {
    let capture = DesktopCapture::try_new(
        1,
        vec![solid_output(
            rect(100, 50, 4, 3),
            Rotation::Identity,
            1_500,
            [8, 9, 10, 255],
        )],
    )
    .unwrap();

    let crop = capture.crop(rect(98, 48, 5, 5)).unwrap();

    assert_eq!(crop.rect, rect(100, 50, 3, 3));
    assert_eq!(crop.dimensions(), (3, 3));
    assert_eq!(crop.pixel(2, 2), Some([8, 9, 10, 255]));
}

#[test]
fn desktop_capture_rejects_overlapping_outputs() {
    let result = DesktopCapture::try_new(
        1,
        vec![
            solid_output(rect(0, 0, 4, 4), Rotation::Identity, 1_000, [0, 0, 0, 255]),
            solid_output(rect(3, 0, 4, 4), Rotation::Identity, 1_000, [0, 0, 0, 255]),
        ],
    );

    assert!(matches!(
        result,
        Err(CaptureError::OverlappingOutputs { .. })
    ));
}

#[test]
fn frames_and_crops_reject_zero_area_and_stride_overflow() {
    let zero = CapturedOutput::try_new(rect(0, 0, 0, 1), Rotation::Identity, 1_000, 0, Vec::new());
    assert!(matches!(zero, Err(CaptureError::EmptyRegion)));

    let impossible_width = u32::MAX / 4 + 1;
    let impossible_stride =
        RgbaFrame::try_new(rect(i32::MIN, 0, impossible_width, 1), u32::MAX, Vec::new());
    assert!(matches!(
        impossible_stride,
        Err(CaptureError::StrideOverflow { .. })
    ));

    let over_cap = RgbaFrame::try_new(rect(0, 0, 16_384, 8_193), 65_536, Vec::new());
    assert!(matches!(
        over_cap,
        Err(CaptureError::AllocationTooLarge { .. })
    ));

    let capture = DesktopCapture::try_new(
        1,
        vec![solid_output(
            rect(0, 0, 2, 2),
            Rotation::Identity,
            1_000,
            [0, 0, 0, 255],
        )],
    )
    .unwrap();
    assert!(matches!(
        capture.crop(rect(0, 0, 0, 2)),
        Err(CaptureError::EmptyRegion)
    ));
}

#[test]
fn every_supported_rotation_maps_desktop_pixels_to_the_source_surface() {
    let cases = [
        (Rotation::Identity, (2, 1), (2, 1)),
        (Rotation::Rotate90, (2, 1), (1, 0)),
        (Rotation::Rotate180, (2, 1), (0, 0)),
        (Rotation::Rotate270, (2, 1), (0, 2)),
    ];
    for (rotation, desktop_point, source_point) in cases {
        let output = coordinate_output(rect(0, 0, 3, 2), rotation, 1_000);
        let capture = DesktopCapture::try_new(1, vec![output]).unwrap();
        let pixel = capture
            .crop(rect(desktop_point.0, desktop_point.1, 1, 1))
            .unwrap()
            .pixel(0, 0);
        assert_eq!(
            pixel,
            Some(expected_source_pixel(source_point.0, source_point.1)),
            "rotation {rotation:?}"
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires an interactive Windows desktop"]
fn live_dxgi_capture_returns_an_owned_crop() {
    use rust_platform_windows::screen_capture::DxgiDesktopCaptureBackend;

    let capture = DxgiDesktopCaptureBackend.capture().unwrap();
    let bounds = capture.bounds();
    let crop = capture
        .crop(rect(
            bounds.x,
            bounds.y,
            bounds.width.min(8),
            bounds.height.min(8),
        ))
        .unwrap();
    assert_eq!(
        crop.pixels.len(),
        crop.stride as usize * crop.rect.height as usize
    );
}

#[derive(Clone)]
struct FakeBackend {
    name: &'static str,
    calls: Arc<Mutex<Vec<&'static str>>>,
    result: Result<DesktopCapture, CaptureError>,
}

impl DesktopCaptureBackend for FakeBackend {
    fn capture(&self) -> Result<DesktopCapture, CaptureError> {
        self.calls.lock().unwrap().push(self.name);
        self.result.clone()
    }
}

fn one_pixel_capture() -> DesktopCapture {
    DesktopCapture::try_new(
        1,
        vec![solid_output(
            rect(0, 0, 1, 1),
            Rotation::Identity,
            1_000,
            [1, 2, 3, 255],
        )],
    )
    .unwrap()
}

#[test]
fn fallback_runs_only_after_a_temporary_dxgi_failure() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let dxgi = FakeBackend {
        name: "dxgi",
        calls: Arc::clone(&calls),
        result: Err(CaptureError::TemporaryUnavailable("busy".into())),
    };
    let gdi = FakeBackend {
        name: "gdi",
        calls: Arc::clone(&calls),
        result: Ok(one_pixel_capture()),
    };

    let capture = capture_with_fallback(&dxgi, &gdi).unwrap();

    assert_eq!(
        capture.crop(rect(0, 0, 1, 1)).unwrap().pixel(0, 0),
        Some([1, 2, 3, 255])
    );
    assert_eq!(*calls.lock().unwrap(), vec!["dxgi", "gdi"]);
}

#[test]
fn fallback_does_not_hide_access_lost_or_protected_content() {
    for error in [CaptureError::AccessLost, CaptureError::ProtectedContent] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dxgi = FakeBackend {
            name: "dxgi",
            calls: Arc::clone(&calls),
            result: Err(error.clone()),
        };
        let gdi = FakeBackend {
            name: "gdi",
            calls: Arc::clone(&calls),
            result: Ok(one_pixel_capture()),
        };

        assert_eq!(capture_with_fallback(&dxgi, &gdi), Err(error));
        assert_eq!(*calls.lock().unwrap(), vec!["dxgi"]);
    }
}

#[test]
fn repeated_capture_release_keeps_no_owned_frame_alive() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let backend = FakeBackend {
        name: "dxgi",
        calls: Arc::clone(&calls),
        result: Ok(one_pixel_capture()),
    };

    for _ in 0..200 {
        let capture = backend.capture().unwrap();
        assert_eq!(capture.owned_frame_bytes(), 4);
        drop(capture);
    }

    assert_eq!(calls.lock().unwrap().len(), 200);
}
