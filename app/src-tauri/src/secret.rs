//! Stores the Anthropic API key encrypted with Windows DPAPI (only the same Windows user can read it).

use anyhow::{anyhow, Result};
use base64::Engine;

#[cfg(windows)]
fn transform(data: &[u8], protect: bool) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if protect {
            CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)?;
        } else {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)?;
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        Ok(result)
    }
}

#[cfg(not(windows))]
fn transform(data: &[u8], _protect: bool) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}

pub fn protect(secret: &str) -> Result<String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(transform(secret.as_bytes(), true)?))
}

pub fn unprotect(stored: &str) -> Result<String> {
    if stored.is_empty() {
        return Err(anyhow!("aucune clé enregistrée"));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(stored)?;
    Ok(String::from_utf8(transform(&bytes, false)?)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let stored = super::protect("sk-ant-test-123").unwrap();
        assert!(!stored.contains("sk-ant"));
        assert_eq!(super::unprotect(&stored).unwrap(), "sk-ant-test-123");
    }
}
