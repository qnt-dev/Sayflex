use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

pub const DISPLAY_NAME: &str = "StyleFlex";
pub const DATA_DIRECTORY_NAME: &str = "StyleFlex";
pub const USER_AGENT_PRODUCT: &str = "StyleFlex/0.5";

const LEGACY_DATA_DIRECTORY_NAME: &str = "KeyboopRust";

pub fn data_root() -> PathBuf {
    env::var_os("LOCALAPPDATA").map_or_else(
        || {
            env::current_dir()
                .unwrap_or_default()
                .join(".styleflex-data")
        },
        |root| {
            let root = Path::new(&root);
            let current = root.join(DATA_DIRECTORY_NAME);
            let legacy = root.join(LEGACY_DATA_DIRECTORY_NAME);
            if current.exists() || !legacy.exists() {
                current
            } else {
                // If an older running process temporarily prevents the atomic
                // rename, keep using its data instead of starting empty.
                legacy
            }
        },
    )
}

/// Moves the previous Rust application's data directory into the canonical
/// StyleFlex location. The rename is atomic on the LOCALAPPDATA volume, never
/// overwrites an existing StyleFlex directory, and keeps upgrades lossless.
pub fn migrate_legacy_data_root() -> io::Result<bool> {
    let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return Ok(false);
    };
    let current = local_app_data.join(DATA_DIRECTORY_NAME);
    let legacy = local_app_data.join(LEGACY_DATA_DIRECTORY_NAME);
    migrate_data_root(&legacy, &current)
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

    use super::migrate_data_root;

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
