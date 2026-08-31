//! Encryption of stored secrets with the Windows data protection API.
//!
//! Keys for transcription services are the only long-lived secrets the
//! application holds. They are encrypted with `CryptProtectData` in user scope,
//! so the ciphertext is useless on another machine and to another Windows
//! account, and they live in their own file rather than in `settings.json`:
//! settings get copied into bug reports and screenshots, and a key must not
//! travel with them.
//!
//! The boundary this does *not* cross is stated plainly in SECURITY.md: a
//! process already running as the same user can ask Windows to decrypt the
//! same blob. Protecting against that is not possible for a program that must
//! decrypt the key by itself, every time, without asking the user.

use std::{ffi::c_void, io, ptr};

use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData},
};

/// Binds a protected blob to this application, so a blob taken from another
/// program's store cannot be decrypted by this one and the other way round.
const ENTROPY: &[u8] = b"SayFlex.transcription-credentials.v1";

/// Encrypts for the current Windows user.
pub fn protect(plaintext: &[u8]) -> io::Result<Vec<u8>> {
    transform(plaintext, Direction::Protect)
}

/// Decrypts a blob produced by [`protect`] on this machine and account.
pub fn unprotect(ciphertext: &[u8]) -> io::Result<Vec<u8>> {
    transform(ciphertext, Direction::Unprotect)
}

#[derive(Clone, Copy)]
enum Direction {
    Protect,
    Unprotect,
}

fn transform(input: &[u8], direction: Direction) -> io::Result<Vec<u8>> {
    // A zero-length input would give CryptProtectData a null pointer, which it
    // reads as "no data" rather than "empty data".
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let input_blob = blob(input);
    let entropy_blob = blob(ENTROPY);
    let mut output_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };

    let succeeded = unsafe {
        match direction {
            Direction::Protect => CryptProtectData(
                &input_blob,
                ptr::null(),
                &entropy_blob,
                ptr::null(),
                ptr::null(),
                0,
                &mut output_blob,
            ),
            Direction::Unprotect => CryptUnprotectData(
                &input_blob,
                ptr::null_mut(),
                &entropy_blob,
                ptr::null(),
                ptr::null(),
                0,
                &mut output_blob,
            ),
        }
    };

    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: the call succeeded, so the blob points at cbData readable bytes
    // allocated by the API, which must be released with LocalFree.
    let result = unsafe {
        std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec()
    };
    unsafe {
        LocalFree(output_blob.pbData.cast::<c_void>());
    }
    Ok(result)
}

fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        // The API takes a mutable pointer but does not write through it for
        // these two calls.
        pbData: bytes.as_ptr().cast_mut(),
    }
}

/// Adapter that lets the shared credential store use DPAPI without depending
/// on Windows itself.
pub struct DpapiCipher;

impl rust_app_core::credentials::SecretCipher for DpapiCipher {
    fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>> {
        protect(plaintext)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> io::Result<Vec<u8>> {
        unprotect(ciphertext)
    }
}

#[cfg(test)]
mod tests {
    use super::{protect, unprotect};

    #[test]
    fn a_secret_survives_a_round_trip_and_is_not_stored_in_the_clear() {
        let secret = b"gsk_a-transcription-service-key";

        let protected = protect(secret).expect("DPAPI is available to a user session");

        assert_ne!(protected.as_slice(), secret.as_slice());
        assert!(
            !protected
                .windows(secret.len())
                .any(|window| window == secret),
            "the plaintext must not appear inside the ciphertext"
        );
        assert_eq!(unprotect(&protected).unwrap(), secret);
    }

    #[test]
    fn a_blob_from_another_application_is_refused() {
        let mut protected = protect(b"a secret").unwrap();
        // Corrupting the blob stands in for one produced with other entropy.
        let last = protected.len() - 1;
        protected[last] ^= 0xff;

        assert!(unprotect(&protected).is_err());
    }

    #[test]
    fn an_empty_secret_is_handled_without_calling_the_api() {
        assert!(protect(b"").unwrap().is_empty());
        assert!(unprotect(b"").unwrap().is_empty());
    }
}

#[cfg(test)]
mod store_tests {
    use rust_app_core::credentials::CredentialStore;
    use tempfile::tempdir;

    use super::DpapiCipher;

    #[test]
    fn a_key_stored_through_dpapi_reads_back_on_this_account() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("credentials.dat");

        let mut store = CredentialStore::open(path.clone(), DpapiCipher);
        store.set("groq", "gsk_protected_by_windows_1234").unwrap();

        let written = std::fs::read(&path).unwrap();
        assert!(
            !written.windows(4).any(|window| window == b"gsk_"),
            "the key must not be readable in the file"
        );
        assert_eq!(
            CredentialStore::open(path, DpapiCipher).secret("groq"),
            Some("gsk_protected_by_windows_1234")
        );
    }
}
