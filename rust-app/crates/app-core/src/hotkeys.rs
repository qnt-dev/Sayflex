use std::{collections::HashMap, fmt};

use rust_protocol::{HotkeyAction, HotkeyBindings, HotkeyRejection, HotkeySpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyValidationError {
    pub action: HotkeyAction,
    pub reason: HotkeyRejection,
    pub message: String,
}

impl fmt::Display for HotkeyValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HotkeyValidationError {}

#[must_use]
pub fn format_hotkey(spec: HotkeySpec) -> String {
    let mut parts = Vec::with_capacity(5);
    if spec.modifiers.control {
        parts.push("Ctrl".to_owned());
    }
    if spec.modifiers.alt {
        parts.push("Alt".to_owned());
    }
    if spec.modifiers.shift {
        parts.push("Shift".to_owned());
    }
    if spec.modifiers.windows {
        parts.push("Win".to_owned());
    }
    parts.push(key_name(spec.virtual_key));
    parts.join(" + ")
}

pub fn validate(bindings: &HotkeyBindings) -> Result<(), HotkeyValidationError> {
    if !bindings.pause_resume.enabled {
        return Err(invalid(
            HotkeyAction::PauseResume,
            "Pause/resume must remain enabled so paused input can be restored.",
        ));
    }

    let mut occupied = HashMap::<HotkeySpec, HotkeyAction>::new();
    for action in HotkeyAction::ALL {
        let binding = bindings.binding(action);
        if !binding.enabled {
            continue;
        }
        if binding.spec.modifiers.is_empty()
            || binding.spec.virtual_key == 0
            || is_modifier_key(binding.spec.virtual_key)
        {
            return Err(invalid(
                action,
                "A shortcut needs a non-modifier key and at least one modifier.",
            ));
        }
        if let Some(previous) = occupied.insert(binding.spec, action) {
            return Err(HotkeyValidationError {
                action,
                reason: HotkeyRejection::Duplicate,
                message: format!(
                    "{} is already assigned to {:?}.",
                    format_hotkey(binding.spec),
                    previous
                ),
            });
        }
    }
    Ok(())
}

fn invalid(action: HotkeyAction, message: &str) -> HotkeyValidationError {
    HotkeyValidationError {
        action,
        reason: HotkeyRejection::Invalid,
        message: message.to_owned(),
    }
}

const fn is_modifier_key(key: u16) -> bool {
    matches!(key, 0x10..=0x12 | 0x5b..=0x5c | 0xa0..=0xa5)
}

fn key_name(key: u16) -> String {
    match key {
        0x20 => "Space".to_owned(),
        0x09 => "Tab".to_owned(),
        0x0d => "Enter".to_owned(),
        0x1b => "Esc".to_owned(),
        0x25 => "Left".to_owned(),
        0x26 => "Up".to_owned(),
        0x27 => "Right".to_owned(),
        0x28 => "Down".to_owned(),
        0x70..=0x87 => format!("F{}", key - 0x6f),
        value if matches!(value, 0x30..=0x39 | 0x41..=0x5a) => char::from(value as u8).to_string(),
        _ => format!("VK 0x{key:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::{HotkeyAction, HotkeyBindings, HotkeyRejection, HotkeySpec};

    use super::{format_hotkey, validate};

    #[test]
    fn duplicate_enabled_chords_name_the_second_action() {
        let mut bindings = HotkeyBindings::default();
        bindings.translate.spec = bindings.insert_last.spec;
        bindings.translate.enabled = true;

        let error = validate(&bindings).unwrap_err();

        assert_eq!(error.action, HotkeyAction::Translate);
        assert_eq!(error.reason, HotkeyRejection::Duplicate);
    }

    #[test]
    fn disabled_chords_do_not_conflict_with_live_actions() {
        let mut bindings = HotkeyBindings::default();
        bindings.translate.spec = bindings.insert_last.spec;
        bindings.translate.enabled = false;

        assert_eq!(validate(&bindings), Ok(()));
    }

    #[test]
    fn a_trigger_without_a_modifier_is_rejected() {
        let mut bindings = HotkeyBindings::default();
        bindings.translate.spec.modifiers = rust_protocol::HotkeyModifiers {
            control: false,
            alt: false,
            shift: false,
            windows: false,
        };
        bindings.translate.enabled = true;

        let error = validate(&bindings).unwrap_err();

        assert_eq!(error.action, HotkeyAction::Translate);
        assert_eq!(error.reason, HotkeyRejection::Invalid);
    }

    #[test]
    fn a_modifier_cannot_also_be_the_trigger() {
        let mut bindings = HotkeyBindings::default();
        bindings.dictate.spec = HotkeySpec::control_alt(0x11);

        assert_eq!(
            validate(&bindings).unwrap_err().action,
            HotkeyAction::Dictate
        );
    }

    #[test]
    fn pause_cannot_be_disabled_because_it_is_the_recovery_chord() {
        let mut bindings = HotkeyBindings::default();
        bindings.pause_resume.enabled = false;

        assert_eq!(
            validate(&bindings).unwrap_err().action,
            HotkeyAction::PauseResume
        );
    }

    #[test]
    fn display_is_canonical_and_independent_of_keyboard_layout() {
        let bindings = HotkeyBindings::default();

        assert_eq!(format_hotkey(bindings.translate.spec), "Ctrl + Alt + F");
        assert_eq!(format_hotkey(bindings.dictate.spec), "Ctrl + Alt + Space");
    }
}
