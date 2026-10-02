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
        if !well_formed(binding.spec) {
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

/// Repairs only the parts of a stored configuration that `validate` rejects.
///
/// A settings file can hold a configuration the validator refuses — older
/// builds saved what Windows accepted instead of what the user chose, so a
/// recovery chord another application held was written back as disabled.
/// Replacing the whole table would silently discard every custom chord the
/// user set, so each defect is repaired where it is: pause is re-enabled,
/// a malformed chord returns to its default, and an action whose chord is
/// already taken returns to its default chord or, failing that, is switched
/// off. Pause claims its chord first because it is the one that must stay.
#[must_use]
pub fn repair(bindings: &HotkeyBindings) -> HotkeyBindings {
    if validate(bindings).is_ok() {
        return bindings.clone();
    }
    let defaults = HotkeyBindings::default();
    let mut repaired = bindings.clone();
    repaired.pause_resume.enabled = true;

    let order = std::iter::once(HotkeyAction::PauseResume).chain(
        HotkeyAction::ALL
            .into_iter()
            .filter(|action| *action != HotkeyAction::PauseResume),
    );
    let mut occupied = HashMap::<HotkeySpec, HotkeyAction>::new();
    for action in order {
        let default = defaults.binding(action).spec;
        let binding = repaired.binding_mut(action);
        if !binding.enabled {
            continue;
        }
        if !well_formed(binding.spec) {
            binding.spec = default;
        }
        if occupied.contains_key(&binding.spec) {
            binding.spec = default;
        }
        if occupied.contains_key(&binding.spec) {
            if action == HotkeyAction::PauseResume {
                // Unreachable: pause is placed first, but never leave it off.
                continue;
            }
            binding.enabled = false;
            continue;
        }
        occupied.insert(binding.spec, action);
    }

    if validate(&repaired).is_ok() {
        repaired
    } else {
        defaults
    }
}

fn well_formed(spec: HotkeySpec) -> bool {
    !spec.modifiers.is_empty() && spec.virtual_key != 0 && !is_modifier_key(spec.virtual_key)
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

    use super::{format_hotkey, repair, validate};

    fn custom() -> HotkeyBindings {
        let mut bindings = HotkeyBindings::default();
        bindings.dictate.spec = HotkeySpec::control_alt(u16::from(b'D'));
        bindings.insert_last.spec = HotkeySpec::control_alt(u16::from(b'I'));
        bindings.translate.enabled = true;
        bindings.translate.spec = HotkeySpec::control_alt(u16::from(b'T'));
        bindings
    }

    #[test]
    fn repair_keeps_a_valid_configuration_untouched() {
        let bindings = custom();

        assert_eq!(repair(&bindings), bindings);
    }

    #[test]
    fn repair_re_enables_pause_and_keeps_every_custom_chord() {
        // The exact shape older builds wrote when Windows refused Ctrl+Alt+P.
        let mut stored = custom();
        stored.pause_resume.enabled = false;

        let repaired = repair(&stored);

        assert!(repaired.pause_resume.enabled);
        assert_eq!(repaired.pause_resume.spec, stored.pause_resume.spec);
        for action in HotkeyAction::ALL {
            if action != HotkeyAction::PauseResume {
                assert_eq!(
                    repaired.binding(action),
                    stored.binding(action),
                    "{action:?}"
                );
            }
        }
        assert_eq!(validate(&repaired), Ok(()));
    }

    #[test]
    fn repair_returns_only_a_malformed_chord_to_its_default() {
        let mut stored = custom();
        stored.insert_last.spec.modifiers = rust_protocol::HotkeyModifiers {
            control: false,
            alt: false,
            shift: false,
            windows: false,
        };

        let repaired = repair(&stored);

        assert_eq!(
            repaired.insert_last.spec,
            HotkeyBindings::default().insert_last.spec
        );
        assert_eq!(repaired.dictate, stored.dictate);
        assert_eq!(repaired.translate, stored.translate);
        assert_eq!(validate(&repaired), Ok(()));
    }

    #[test]
    fn repair_resolves_a_duplicate_without_touching_the_first_owner() {
        let mut stored = custom();
        stored.translate.spec = stored.dictate.spec;

        let repaired = repair(&stored);

        assert_eq!(repaired.dictate, stored.dictate);
        assert_eq!(
            repaired.translate.spec,
            HotkeyBindings::default().translate.spec
        );
        assert!(repaired.translate.enabled);
        assert_eq!(validate(&repaired), Ok(()));
    }

    #[test]
    fn pause_keeps_its_chord_when_another_action_took_it() {
        let mut stored = custom();
        stored.pause_resume.enabled = false;
        stored.translate.spec = stored.pause_resume.spec;

        let repaired = repair(&stored);

        assert!(repaired.pause_resume.enabled);
        assert_eq!(
            repaired.pause_resume.spec,
            HotkeyBindings::default().pause_resume.spec
        );
        assert_ne!(repaired.translate.spec, repaired.pause_resume.spec);
        assert_eq!(validate(&repaired), Ok(()));
    }

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
