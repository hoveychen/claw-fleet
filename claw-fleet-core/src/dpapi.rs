//! Windows DPAPI helpers for reading Electron `safeStorage` blobs.
//!
//! Claude Desktop App on Windows stores its OAuth token cache encrypted via
//! Electron `safeStorage`, which on Windows is a thin wrapper around the Win32
//! `CryptUnprotectData` API (DPAPI) keyed to the current user. The encoded
//! payload is `base64( "v10" || dpapi_ciphertext )` — the leading 3 bytes are
//! Chromium's `os_crypt` version tag (`v10`, `v11`, …); what follows is
//! whatever the calling code passed to `safeStorage.encryptString()`.
//!
//! Gated on `#[cfg(windows)]` at the module level so callers may freely
//! reference the symbols inside their own `cfg(windows)` blocks.

#![cfg(windows)]

use base64::Engine;

/// Decrypt an Electron `safeStorage` v10-prefixed, base64-encoded blob and
/// return the raw plaintext bytes. Caller decides how to interpret them
/// (typically UTF-8 JSON).
pub fn decrypt_safe_storage(encoded: &str) -> Result<Vec<u8>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("base64 decode failed: {e}"))?;

    if bytes.len() < 4 {
        return Err(format!("safeStorage blob too short ({} bytes)", bytes.len()));
    }
    // Strip the 3-byte `vNN` version tag. We don't enforce a literal value
    // because Chromium bumps it over time; the tag is not part of the DPAPI
    // ciphertext.
    let ciphertext = &bytes[3..];

    dpapi_unprotect(ciphertext)
}

fn dpapi_unprotect(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: ciphertext.len() as u32,
        pbData: ciphertext.as_ptr().cast_mut(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();

    // SAFETY: `in_blob.pbData` points to a valid `ciphertext.len()`-byte
    // slice; `CryptUnprotectData` only reads it. On success the API
    // heap-allocates `out_blob.pbData` which we copy out then `LocalFree`.
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

        let result =
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out_blob.pbData as *mut _)));
        Ok(result)
    }
}
