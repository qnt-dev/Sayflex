use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub const DISPLAY_NAME: &str = "SayFlex";
pub const DATA_DIRECTORY_NAME: &str = "SayFlex";
pub const USER_AGENT_PRODUCT: &str = concat!("SayFlex/", env!("CARGO_PKG_VERSION"));
pub const TEST_CHANNEL_MARKER: &str = "sayflex-test-channel";
pub const TEST_DATA_DIRECTORY_NAME: &str = "SayFlex-Test-Nvidia";

/// Selected by the local test installer, identically for the UI and workers.
/// Only a fixed profile name is accepted; the marker cannot redirect user data.
pub fn is_test_build() -> bool {
    static TEST_BUILD: OnceLock<bool> = OnceLock::new();
    *TEST_BUILD.get_or_init(|| {
        env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|path| path.join(TEST_CHANNEL_MARKER)))
            .is_some_and(|marker| marker.is_file())
    })
}

pub fn desktop_app_id() -> &'static str {
    if is_test_build() {
        "SayFlex.TestNvidia.Desktop"
    } else {
        "SayFlex.Desktop"
    }
}

// Compatibility-only names. They are deliberately kept out of every visible
// product surface, but upgrades must not strand an existing model or history.
const LEGACY_DATA_DIRECTORY_NAMES: &[&str] = &["StyleFlex", "KeyboopRust"];

pub fn data_root() -> PathBuf {
    env::var_os("LOCALAPPDATA").map_or_else(
        || {
            env::current_dir()
                .unwrap_or_default()
                .join(if is_test_build() {
                    ".sayflex-test-nvidia-data"
                } else {
                    ".sayflex-data"
                })
        },
        |root| data_root_for(Path::new(&root), is_test_build()),
    )
}

fn data_root_for(root: &Path, test_channel: bool) -> PathBuf {
    if test_channel {
        return root.join(TEST_DATA_DIRECTORY_NAME);
    }
    let current = root.join(DATA_DIRECTORY_NAME);
    if current.exists() {
        return current;
    }
    LEGACY_DATA_DIRECTORY_NAMES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.exists())
        .unwrap_or(current)
}

/// Moves the previous Rust application's data directory into the canonical
/// SayFlex location. The rename is atomic on the LOCALAPPDATA volume, never
/// overwrites an existing SayFlex directory, and keeps upgrades lossless.
pub fn migrate_legacy_data_root() -> io::Result<bool> {
    let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return Ok(false);
    };
    migrate_legacy_data_root_for(&local_app_data, is_test_build())
}

fn migrate_legacy_data_root_for(local_app_data: &Path, test_channel: bool) -> io::Result<bool> {
    if test_channel {
        return Ok(false);
    }
    let current = local_app_data.join(DATA_DIRECTORY_NAME);
    for name in LEGACY_DATA_DIRECTORY_NAMES {
        if migrate_data_root(&local_app_data.join(name), &current)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migrate_data_root(legacy: &Path, current: &Path) -> io::Result<bool> {
    if current.exists() || !legacy.exists() {
        return Ok(false);
    }
    fs::rename(legacy, current)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{data_root_for, migrate_data_root, migrate_legacy_data_root_for};

    #[test]
    fn test_profile_never_reads_existing_production_or_legacy_data() {
        let temp = tempdir().unwrap();
        fs::create_dir(temp.path().join("SayFlex")).unwrap();
        fs::create_dir(temp.path().join("StyleFlex")).unwrap();
        assert_eq!(
            data_root_for(temp.path(), true),
            temp.path().join("SayFlex-Test-Nvidia")
        );
        assert_eq!(
            data_root_for(temp.path(), false),
            temp.path().join("SayFlex")
        );
    }

    #[test]
    fn test_profile_never_migrates_legacy_user_data() {
        let temp = tempdir().unwrap();
        let legacy = temp.path().join("StyleFlex");
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("settings.json"), b"private user settings").unwrap();
        assert!(!migrate_legacy_data_root_for(temp.path(), true).unwrap());
        assert_eq!(
            fs::read(legacy.join("settings.json")).unwrap(),
            b"private user settings"
        );
        assert!(!temp.path().join("SayFlex").exists());
    }

    #[test]
    fn migration_moves_existing_data_without_overwriting_current_data() {
        let temp = tempdir().unwrap();
        let legacy = temp.path().join("legacy");
        let current = temp.path().join("current");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("settings.json"), b"settings").unwrap();

        assert!(migrate_data_root(&legacy, &current).unwrap());
        assert_eq!(
            fs::read(current.join("settings.json")).unwrap(),
            b"settings"
        );
        assert!(!legacy.exists());

        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("settings.json"), b"legacy").unwrap();
        assert!(!migrate_data_root(&legacy, &current).unwrap());
        assert_eq!(
            fs::read(current.join("settings.json")).unwrap(),
            b"settings"
        );
    }
}
