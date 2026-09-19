//! In-app developer support destinations and QR rendering.

use std::rc::Rc;

use qrcode::{QrCode, types::Color};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use crate::{SettingsWindow, SupportWindow};

pub(crate) const SUPPORT_QR_SIZE: u32 = 224;
const SUPPORT_QR_BACKGROUND: [u8; 4] = [191, 194, 198, 255];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SupportDestination {
    pub(crate) network: &'static str,
    pub(crate) standard: &'static str,
    pub(crate) address: &'static str,
}

pub(crate) fn support_destinations() -> [SupportDestination; 2] {
    [
        SupportDestination {
            network: "BNB Smart Chain",
            standard: "BEP-20",
            address: "0x391436B8E36Cd3214B620435910571F395dFfb5D",
        },
        SupportDestination {
            network: "TRON",
            standard: "TRC-20",
            address: "TYq8scpdkxb1nxZgnAgxHarM4XMA9ucK4G",
        },
    ]
}

pub(crate) fn render_qr_pixels(
    payload: &str,
) -> Result<SharedPixelBuffer<Rgba8Pixel>, qrcode::types::QrError> {
    const QUIET_ZONE_MODULES: u32 = 4;

    let code = QrCode::new(payload.as_bytes())?;
    let code_width = code.width() as u32;
    let total_modules = code_width + QUIET_ZONE_MODULES * 2;
    let module_scale = SUPPORT_QR_SIZE / total_modules;
    let rendered_size = total_modules * module_scale;
    let code_origin = (SUPPORT_QR_SIZE - rendered_size) / 2 + QUIET_ZONE_MODULES * module_scale;
    let colors = code.to_colors();

    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(SUPPORT_QR_SIZE, SUPPORT_QR_SIZE);
    let bytes = pixels.make_mut_bytes();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.copy_from_slice(&SUPPORT_QR_BACKGROUND);
    }
    for module_y in 0..code_width {
        for module_x in 0..code_width {
            if colors[(module_y * code_width + module_x) as usize] != Color::Dark {
                continue;
            }
            let pixel_x = code_origin + module_x * module_scale;
            let pixel_y = code_origin + module_y * module_scale;
            for y in pixel_y..pixel_y + module_scale {
                for x in pixel_x..pixel_x + module_scale {
                    let offset = ((y * SUPPORT_QR_SIZE + x) * 4) as usize;
                    bytes[offset..offset + 4].copy_from_slice(&[15, 17, 20, 255]);
                }
            }
        }
    }
    Ok(pixels)
}

pub(crate) fn configure_support_window(
    support: &SupportWindow,
) -> Result<(), qrcode::types::QrError> {
    let destinations = support_destinations();
    support.set_bnb_address(destinations[0].address.into());
    support.set_tron_address(destinations[1].address.into());
    support.set_bnb_qr(Image::from_rgba8(render_qr_pixels(
        destinations[0].address,
    )?));
    support.set_tron_qr(Image::from_rgba8(render_qr_pixels(
        destinations[1].address,
    )?));
    Ok(())
}

pub(crate) fn wire_support_with(
    settings: &SettingsWindow,
    support: &SupportWindow,
    copy_address: impl Fn(&str) + 'static,
) {
    let support_weak = support.as_weak();
    settings.on_open_support(move || {
        let Some(support) = support_weak.upgrade() else {
            return;
        };
        support.set_status_text("".into());
        if support.show().is_ok() {
            #[cfg(not(test))]
            crate::configure_standard_app_window(&support, "Developer support");
        }
    });

    let support_weak = support.as_weak();
    support.on_close_requested(move || {
        if let Some(support) = support_weak.upgrade() {
            let _ = support.hide();
        }
    });

    let destinations = support_destinations();
    let copy_address: Rc<dyn Fn(&str)> = Rc::new(copy_address);
    let copy_bnb = Rc::clone(&copy_address);
    support.on_copy_bnb(move || copy_bnb(destinations[0].address));
    let copy_tron = Rc::clone(&copy_address);
    support.on_copy_tron(move || copy_tron(destinations[1].address));
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, rc::Rc};

    use i_slint_backend_testing::ElementHandle;
    use rqrr::PreparedImage;
    use slint::ComponentHandle;

    use super::{
        configure_support_window, render_qr_pixels, support_destinations, wire_support_with,
    };

    #[test]
    fn support_window_opens_copies_exact_addresses_and_closes_without_exiting() {
        i_slint_backend_testing::init_no_event_loop();
        let settings = crate::SettingsWindow::new().expect("settings surface");
        let support = crate::SupportWindow::new().expect("support surface");
        configure_support_window(&support).expect("configure support surface");
        let copied = Rc::new(std::cell::RefCell::new(Vec::new()));
        let copied_from_callback = Rc::clone(&copied);
        wire_support_with(&settings, &support, move |address| {
            copied_from_callback.borrow_mut().push(address.to_owned());
        });

        assert!(!support.window().is_visible());
        settings.invoke_open_support();
        assert!(support.window().is_visible());
        support.invoke_copy_bnb();
        support.invoke_copy_tron();
        assert_eq!(
            copied.borrow().as_slice(),
            [
                "0x391436B8E36Cd3214B620435910571F395dFfb5D",
                "TYq8scpdkxb1nxZgnAgxHarM4XMA9ucK4G",
            ]
        );
        support.invoke_close_requested();
        assert!(!support.window().is_visible());
    }

    #[test]
    fn support_destinations_are_the_two_approved_network_addresses() {
        let destinations = support_destinations();

        assert_eq!(destinations.len(), 2);
        assert_eq!(destinations[0].network, "BNB Smart Chain");
        assert_eq!(destinations[0].standard, "BEP-20");
        assert_eq!(
            destinations[0].address,
            "0x391436B8E36Cd3214B620435910571F395dFfb5D"
        );
        assert_eq!(destinations[1].network, "TRON");
        assert_eq!(destinations[1].standard, "TRC-20");
        assert_eq!(
            destinations[1].address,
            "TYq8scpdkxb1nxZgnAgxHarM4XMA9ucK4G"
        );

        let unique_addresses = destinations
            .iter()
            .map(|destination| destination.address)
            .collect::<HashSet<_>>();
        assert_eq!(unique_addresses.len(), destinations.len());
    }

    #[test]
    fn every_rendered_qr_decodes_to_its_exact_public_address() {
        for destination in support_destinations() {
            let pixels = render_qr_pixels(destination.address).expect("render support QR");
            let width = pixels.width() as usize;
            let height = pixels.height() as usize;
            let bytes = pixels.as_bytes();
            let mut prepared = PreparedImage::prepare_from_greyscale(width, height, |x, y| {
                bytes[(y * width + x) * 4]
            });
            let grids = prepared.detect_grids();

            assert_eq!(grids.len(), 1, "{} QR count", destination.network);
            let (_, decoded) = grids[0].decode().expect("decode rendered support QR");
            assert_eq!(
                decoded, destination.address,
                "{} QR payload",
                destination.network
            );
        }
    }

    #[test]
    fn rendered_qr_uses_the_canvas_gray_and_balanced_quiet_zone() {
        let pixels =
            render_qr_pixels(support_destinations()[0].address).expect("render support QR");
        let width = pixels.width() as usize;
        let height = pixels.height() as usize;
        let bytes = pixels.as_bytes();

        assert_eq!(&bytes[..4], &[191, 194, 198, 255], "QR background");

        let dark = [15, 17, 20, 255];
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0;
        let mut max_y = 0;
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) * 4;
                if bytes[offset..offset + 4] == dark {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }

        assert!((min_x as isize - (width - 1 - max_x) as isize).abs() <= 1);
        assert!((min_y as isize - (height - 1 - max_y) as isize).abs() <= 1);
    }

    #[test]
    fn wallet_card_content_sits_directly_on_one_surface() {
        i_slint_backend_testing::init_no_event_loop();
        let support = crate::SupportWindow::new().expect("support surface");
        configure_support_window(&support).expect("configure support surface");
        support.show().expect("show support surface");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

        let mut cards =
            ElementHandle::find_by_element_type_name(&support, "WalletCard").collect::<Vec<_>>();
        let mut actions =
            ElementHandle::find_by_element_type_name(&support, "CopyAction").collect::<Vec<_>>();
        cards.sort_by(|left, right| {
            left.absolute_position()
                .x
                .total_cmp(&right.absolute_position().x)
        });
        actions.sort_by(|left, right| {
            left.absolute_position()
                .x
                .total_cmp(&right.absolute_position().x)
        });

        assert_eq!(cards.len(), 2, "wallet card count");
        assert_eq!(actions.len(), 2, "copy action count");
        for (card, action) in cards.iter().zip(actions.iter()) {
            assert_eq!(card.size().height, 342.0, "flat wallet card height");
            let card_bottom = card.absolute_position().y + card.size().height;
            let action_bottom = action.absolute_position().y + action.size().height;
            let bottom_gap = card_bottom - action_bottom;
            assert!(
                (bottom_gap - 18.0).abs() <= 0.5,
                "copy action bottom gap is {bottom_gap}, expected 18"
            );
        }
    }
}
