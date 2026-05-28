//! Windows DPAPI helpers for reading Electron `safeStorage` blobs.
//!
//! Claude Desktop App on Windows stores its OAuth token cache encrypted via
//! Electron `safeStorage`, which on Windows is a thin wrapper around the Win32
//! `CryptUnprotectData` API (DPAPI) keyed to the current user. The encrypted
//! payload format is:
//!
//!   base64( "v10" || dpapi_ciphertext )
//!
//! The leading 3 bytes are the literal ASCII string `v10` (Chromium's
//! `os_crypt` versioning byte sequence). What's inside `dpapi_ciphertext` is
//! whatever the calling code passed to `safeStorage.encryptString()` — for
//! Claude Desktop App, that's a JSON document carrying the OAuth tokens.
//!
//! This module is a no-op on non-Windows; the public functions are gated by
//! `#[cfg(windows)]` so callers should themselves be `cfg`-gated.

#![cfg(windows)]

use base64::Engine;

/// Decrypt an Electron `safeStorage` v10-prefixed, base64-encoded blob.
///
/// Returns the raw plaintext bytes (caller decides how to interpret —
/// usually UTF-8 JSON).
pub fn decrypt_safe_storage(encoded: &str) -> Result<Vec<u8>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("base64 decode failed: {e}"))?;

    if bytes.len() < 4 {
        return Err(format!("safeStorage blob too short ({} bytes)", bytes.len()));
    }
    // Strip "v10" prefix. We don't enforce the literal because newer Chromium
    // bumps to "v11" etc.; whatever 3-byte version is present, it's not part
    // of the DPAPI ciphertext.
    let ciphertext = &bytes[3..];

    dpapi_unprotect(ciphertext)
}

/// Thin wrapper over Win32 `CryptUnprotectData` for the current-user scope.
fn dpapi_unprotect(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    // SAFETY: CryptUnprotectData reads `in_blob.pbData` for `cbData` bytes and
    // writes a heap-allocated buffer into `out_blob` that we must `LocalFree`.
    // We don't pass an entropy blob, prompt struct, or description pointer.
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: ciphertext.len() as u32,
        pbData: ciphertext.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptUnprotectData(
            &in_blob as *const _,
            None,
            None,
            None,
            None,
            0,
            &mut out_blob as *mut _,
        )
        .map_err(|e| format!("CryptUnprotectData failed: {e}"))?;

        let result = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(out_blob.pbData as *mut _)));
        Ok(result)
    }
}
