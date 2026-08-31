//! Storage for transcription service keys.
//!
//! Keys live in their own file, encrypted as a whole, and never in
//! `settings.json`: settings are copied into bug reports and screenshots, and
//! a key must not travel with them. The encryption itself is supplied by the
//! platform — on Windows that is DPAPI in user scope — so this module holds
//! only the part worth testing without a machine: what is stored, how it is
//! written, and what the interface is allowed to show.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

/// Encrypts and decrypts for the current user.
pub trait SecretCipher {
    fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>>;
    fn unprotect(&self, ciphertext: &[u8]) -> io::Result<Vec<u8>>;
}

pub struct CredentialStore<C: SecretCipher> {
    path: PathBuf,
    cipher: C,
    secrets: BTreeMap<String, String>,
    /// Set when an existing file could not be read, so the interface can say
    /// so instead of silently presenting an empty list of keys.
    recovery_message: Option<String>,
}

impl<C: SecretCipher> CredentialStore<C> {
    /// Opens the store, or starts an empty one.
    ///
    /// A file that cannot be decrypted is not deleted and not treated as
    /// fatal: it usually means the profile was copied to another machine or
    /// another Windows account, where by design the blob is unreadable. The
    /// user is told, and re-entering the keys replaces it.
    pub fn open(path: PathBuf, cipher: C) -> Self {
        let (secrets, recovery_message) = match fs::read(&path) {
            Ok(bytes) => match cipher
                .unprotect(&bytes)
                .and_then(|plaintext| serde_json::from_slice(&plaintext).map_err(io::Error::other))
            {
                Ok(secrets) => (secrets, None),
                Err(error) => (
                    BTreeMap::new(),
                    Some(format!(
                        "Stored service keys could not be read on this account: {error}"
                    )),
                ),
            },
            Err(_) => (BTreeMap::new(), None),
        };
        Self {
            path,
            cipher,
            secrets,
            recovery_message,
        }
    }

    pub fn take_recovery_message(&mut self) -> Option<String> {
        self.recovery_message.take()
    }

    /// Stores a key, replacing any previous one for that provider. An empty or
    /// blank value removes it, which is what clearing the field in Settings
    /// means.
    pub fn set(&mut self, provider: &str, secret: &str) -> io::Result<()> {
        if secret.trim().is_empty() {
            return self.remove(provider);
        }
        self.secrets
            .insert(provider.to_owned(), secret.trim().to_owned());
        self.save()
    }

    pub fn remove(&mut self, provider: &str) -> io::Result<()> {
        if self.secrets.remove(provider).is_none() {
            return Ok(());
        }
        self.save()
    }

    #[must_use]
    pub fn has(&self, provider: &str) -> bool {
        self.secrets.contains_key(provider)
    }

    #[must_use]
    pub fn secret(&self, provider: &str) -> Option<&str> {
        self.secrets.get(provider).map(String::as_str)
    }

    /// What Settings may display: enough to recognise which key is stored,
    /// never enough to use it.
    #[must_use]
    pub fn hint(&self, provider: &str) -> Option<String> {
        let secret = self.secrets.get(provider)?;
        let visible: String = secret
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // A key short enough for its tail to give it away is shown entirely
        // masked.
        if secret.chars().count() <= 8 {
            return Some("•".repeat(8));
        }
        Some(format!("••••{visible}"))
    }

    #[must_use]
    pub fn providers(&self) -> Vec<&str> {
        self.secrets.keys().map(String::as_str).collect()
    }

    fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        if self.secrets.is_empty() {
            // Nothing left to protect: remove the file rather than leave an
            // encrypted empty map behind.
            return match fs::remove_file(&self.path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            };
        }
        let plaintext = serde_json::to_vec(&self.secrets).map_err(io::Error::other)?;
        let ciphertext = self.cipher.protect(&plaintext)?;
        write_atomically(&self.path, &ciphertext)
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use tempfile::tempdir;

    use super::{CredentialStore, SecretCipher};

    /// Stands in for DPAPI: reversible, and different enough from the input
    /// that a test can tell ciphertext from plaintext.
    struct XorCipher;

    impl SecretCipher for XorCipher {
        fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>> {
            Ok(plaintext.iter().map(|byte| byte ^ 0x5a).collect())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> io::Result<Vec<u8>> {
            Ok(ciphertext.iter().map(|byte| byte ^ 0x5a).collect())
        }
    }

    struct BrokenCipher;

    impl SecretCipher for BrokenCipher {
        fn protect(&self, _: &[u8]) -> io::Result<Vec<u8>> {
            Ok(vec![0])
        }

        fn unprotect(&self, _: &[u8]) -> io::Result<Vec<u8>> {
            Err(io::Error::other("this blob belongs to another account"))
        }
    }

    #[test]
    fn a_key_survives_a_restart_and_never_touches_the_disk_in_the_clear() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("credentials.dat");

        let mut store = CredentialStore::open(path.clone(), XorCipher);
        store.set("groq", "gsk_secret_value_1234").unwrap();

        let written = std::fs::read(&path).unwrap();
        assert!(
            !written.windows(9).any(|window| window == b"gsk_secre"),
            "the key must not be readable in the file"
        );

        let reopened = CredentialStore::open(path, XorCipher);
        assert_eq!(reopened.secret("groq"), Some("gsk_secret_value_1234"));
    }

    #[test]
    fn settings_can_show_which_key_is_stored_without_showing_the_key() {
        let directory = tempdir().unwrap();
        let mut store = CredentialStore::open(directory.path().join("c.dat"), XorCipher);
        store.set("groq", "gsk_long_secret_9876").unwrap();
        store.set("azure", "short").unwrap();

        assert_eq!(store.hint("groq").as_deref(), Some("••••9876"));
        assert_eq!(
            store.hint("azure").as_deref(),
            Some("••••••••"),
            "a short key would be given away by its tail"
        );
        assert_eq!(store.hint("deepgram"), None);
    }

    #[test]
    fn clearing_the_field_removes_the_key_and_finally_the_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("credentials.dat");
        let mut store = CredentialStore::open(path.clone(), XorCipher);

        store.set("groq", "gsk_value_abcdef").unwrap();
        assert!(path.exists());

        store.set("groq", "   ").unwrap();
        assert!(!store.has("groq"));
        assert!(
            !path.exists(),
            "no encrypted leftovers once nothing is kept"
        );
    }

    #[test]
    fn a_file_from_another_account_is_reported_rather_than_discarded() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("credentials.dat");
        std::fs::write(&path, b"not decryptable here").unwrap();

        let mut store = CredentialStore::open(path.clone(), BrokenCipher);

        assert!(store.take_recovery_message().is_some());
        assert!(store.providers().is_empty());
        assert!(path.exists(), "the unreadable file is left alone");
    }

    #[test]
    fn keys_are_stored_per_provider() {
        let directory = tempdir().unwrap();
        let mut store = CredentialStore::open(directory.path().join("c.dat"), XorCipher);

        store.set("groq", "one_key_value").unwrap();
        store.set("cloudflare", "another_key_value").unwrap();

        assert_eq!(store.providers(), ["cloudflare", "groq"]);
        assert_eq!(store.secret("groq"), Some("one_key_value"));
        assert!(store.has("cloudflare"));
    }
}
