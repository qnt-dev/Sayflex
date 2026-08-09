use rust_protocol::{InsertionDelivery, InsertionMethod, TargetSnapshot};

use crate::{inject, target, wezterm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryReceipt {
    pub method: InsertionMethod,
    pub delivery: InsertionDelivery,
}

/// Routes insertion by capability. Automatic insertion never borrows the
/// user's clipboard: a target-specific acknowledged adapter is preferred, and
/// synthetic Unicode input is the correctness-preserving universal fallback.
pub fn deliver(
    target_snapshot: TargetSnapshot,
    text: &str,
    add_trailing_space: bool,
) -> Result<DeliveryReceipt, String> {
    let payload = if add_trailing_space {
        format!("{text} ")
    } else {
        text.to_owned()
    };

    // The override is used only by the explicitly opt-in isolated smoke mode;
    // it lets the packaged binary exercise the universal fallback even when
    // the fixture itself is a native Edit control.
    let force_unicode =
        std::env::var("SAYFLEX_INSERTION_SMOKE_FORCE_UNICODE").as_deref() == Ok("1");

    if !force_unicode && target::is_wezterm_target(target_snapshot) {
        // Once the CLI adapter has been invoked its result must not be followed
        // by another transport: a timeout can be delivery-ambiguous and an
        // automatic fallback could duplicate the transcript.
        wezterm::send_text(target_snapshot, &payload)?;
        return Ok(DeliveryReceipt {
            method: InsertionMethod::WezTermCli,
            delivery: InsertionDelivery::AdapterAcknowledged,
        });
    }

    if !force_unicode
        && inject::insert_native_edit(target_snapshot, text, add_trailing_space)
            .map_err(|error| format!("native edit adapter failed: {error}"))?
    {
        return Ok(DeliveryReceipt {
            method: InsertionMethod::NativeEditMessage,
            delivery: InsertionDelivery::AdapterAcknowledged,
        });
    }

    inject::insert_text(target_snapshot, text, add_trailing_space)
        .map_err(|error| format!("Unicode input adapter failed: {error}"))?;
    Ok(DeliveryReceipt {
        method: InsertionMethod::UnicodeInput,
        delivery: InsertionDelivery::InputStreamAccepted,
    })
}
