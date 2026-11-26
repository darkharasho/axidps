//! Authenticode check for a downloaded installer (Windows only).

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{BOOL, HANDLE, HWND};
use windows::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvCertFromChain,
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::core::PCWSTR;

/// Whether `path` carries a valid Authenticode signature whose signer's name hashes
/// (SHA-256, trimmed) to `publisher_sha256`. Revocation is not checked: that needs the network, and offline installs must
/// not fail.
pub fn authenticode_ok(path: &Path, publisher_sha256: &[u8; 32]) -> bool {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `file`, `data` and `wide` outlive both calls; the signer name is read between
    // VERIFY and CLOSE, while the state data it points into is still alive.
    unsafe {
        let status = WinVerifyTrust(
            HWND::default(),
            &mut action,
            &mut data as *mut WINTRUST_DATA as *mut _,
        );
        let ok = status == 0 && signer_is(data.hWVTStateData, publisher_sha256);
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            &mut data as *mut WINTRUST_DATA as *mut _,
        );
        ok
    }
}

/// Whether the leaf certificate of the first signer has a display name (CN) whose SHA-256 is `publisher_sha256`.
///
/// # Safety
/// `state` must be the `hWVTStateData` of a successful `WTD_STATEACTION_VERIFY` call that has
/// not been closed yet.
unsafe fn signer_is(state: HANDLE, publisher_sha256: &[u8; 32]) -> bool {
    unsafe {
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return false;
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, BOOL(0), 0);
        if signer.is_null() {
            return false;
        }
        let cert = WTHelperGetProvCertFromChain(signer, 0);
        if cert.is_null() || (*cert).pCert.is_null() {
            return false;
        }
        let mut name = [0u16; 512];
        let len = CertGetNameStringW(
            (*cert).pCert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            None,
            Some(&mut name),
        ) as usize;
        // `len` counts the terminating NUL.
        crate::update::publisher_matches(
            &String::from_utf16_lossy(&name[..len.saturating_sub(1).min(name.len())]),
            publisher_sha256,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_file_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("axidps-Setup.exe");
        std::fs::write(&file, b"not an installer").unwrap();
        assert!(!authenticode_ok(&file, &[0; 32]));
    }
}
