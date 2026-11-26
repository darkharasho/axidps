//! How the axidps game DLL identifies itself: an ASCII marker embedded in the file, found by a
//! byte search (no PE parser needed).

/// Prefix of the marker. The DLL embeds `axidps-dll-version=<version>\0`.
pub const MARKER_PREFIX: &[u8] = b"axidps-dll-version=";

/// The version this build carries: `AXIDPS_VERSION` at build time (empty counts as unset), else
/// the workspace crate version.
pub const fn build_version() -> &'static str {
    match option_env!("AXIDPS_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => env!("CARGO_PKG_VERSION"),
    }
}

/// The version text after an occurrence of `MARKER_PREFIX`, up to `\0`, if it is valid ASCII
/// semver. Every occurrence is tried; the first valid one wins.
pub fn find_dll_version(bytes: &[u8]) -> Option<String> {
    let n = MARKER_PREFIX.len();
    bytes
        .windows(n)
        .enumerate()
        .filter(|(_, w)| *w == MARKER_PREFIX)
        .find_map(|(i, _)| {
            let rest = &bytes[i + n..];
            let end = rest.iter().position(|&b| b == 0)?;
            let text = std::str::from_utf8(&rest[..end]).ok()?;
            is_semver(text).then(|| text.to_string())
        })
}

/// The marker's export name. Builds since the version marker export it.
pub const ID_EXPORT: &str = "AXIDPS_DLL_ID";

/// An export every axidps DLL has had since the first proxy, marker or not (the forwarding
/// table's first slot). System32's d3d11.dll, DXVK, arcdps and Nexus do not export it.
pub const PROXY_EXPORT: &str = "pfn_D3D11CreateDevice";

/// Whether `bytes` is an axidps game DLL of any age: it carries the version marker, or its PE
/// export table names [`ID_EXPORT`] or [`PROXY_EXPORT`] (builds from before the marker).
pub fn is_axidps_dll(bytes: &[u8]) -> bool {
    find_dll_version(bytes).is_some()
        || pe_export_names(bytes)
            .is_some_and(|names| names.iter().any(|n| n == ID_EXPORT || n == PROXY_EXPORT))
}

/// The export names of a PE image (PE32 or PE32+) given as the file's bytes, or `None` if it
/// is not a PE file or has no export table. Malformed offsets give `None`, never a panic.
pub fn pe_export_names(bytes: &[u8]) -> Option<Vec<String>> {
    let u16_at = |o: usize| -> Option<u16> {
        Some(u16::from_le_bytes(bytes.get(o..o.checked_add(2)?)?.try_into().ok()?))
    };
    let u32_at = |o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(o..o.checked_add(4)?)?.try_into().ok()?))
    };
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(0x3c)? as usize;
    if bytes.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let coff = pe + 4;
    let sections = u16_at(coff + 2)? as usize;
    let opt_size = u16_at(coff + 16)? as usize;
    let opt = coff + 20;
    let (count_at, dir_at) = match u16_at(opt)? {
        0x10b => (opt + 92, opt + 96),
        0x20b => (opt + 108, opt + 112),
        _ => return None,
    };
    if u32_at(count_at)? == 0 {
        return None;
    }
    let export_rva = u32_at(dir_at)?;
    if export_rva == 0 {
        return None;
    }
    let table = opt.checked_add(opt_size)?;
    let to_offset = |rva: u32| -> Option<usize> {
        (0..sections.min(96)).find_map(|i| {
            let sec = table + i * 40;
            let vsize = u32_at(sec + 8)?;
            let va = u32_at(sec + 12)?;
            let raw_size = u32_at(sec + 16)?;
            let raw = u32_at(sec + 20)?;
            let span = vsize.max(raw_size);
            (rva >= va && rva - va < span).then(|| raw as usize + (rva - va) as usize)
        })
    };
    let dir = to_offset(export_rva)?;
    let count = (u32_at(dir + 24)? as usize).min(65536);
    let names = to_offset(u32_at(dir + 32)?)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = to_offset(u32_at(names + i * 4)?)?;
        let rest = bytes.get(start..)?;
        let end = rest.iter().take(512).position(|&b| b == 0)?;
        out.push(String::from_utf8_lossy(&rest[..end]).into_owned());
    }
    Some(out)
}

/// `MAJOR.MINOR.PATCH`, optional `-prerelease` and `+build` ([0-9A-Za-z.-]).
fn is_semver(s: &str) -> bool {
    if !s.is_ascii() {
        return false;
    }
    let (rest, build) = match s.split_once('+') {
        Some((r, b)) => (r, Some(b)),
        None => (s, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (rest, None),
    };
    let ident_ok = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(ident_ok)
        && build.is_none_or(ident_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_marker_version() {
        let mut b = vec![0u8; 64];
        b.extend_from_slice(b"axidps-dll-version=0.2.0-rc.1\0junk");
        assert_eq!(find_dll_version(&b).as_deref(), Some("0.2.0-rc.1"));
    }
    #[test]
    fn no_marker_is_none() {
        assert_eq!(find_dll_version(b"d3d11 arcdps"), None);
    }
    #[test]
    fn garbage_after_marker_is_none() {
        assert_eq!(find_dll_version(b"axidps-dll-version=\xff\xfe\0"), None);
    }
    #[test]
    fn skips_earlier_invalid_occurrence() {
        assert_eq!(
            find_dll_version(b"..axidps-dll-version=\0 ... axidps-dll-version=0.2.0\0").as_deref(),
            Some("0.2.0")
        );
    }
    /// A minimal PE32+ image with one section holding an export table that names `names`.
    pub(crate) fn pe_with_exports(names: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        b[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // one section
        b[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes()); // PE32+ optional header
        let opt = coff + 20;
        b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        b[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        // Section .edata: RVA 0x1000, file offset 0x200, size 0x200.
        let sec = opt + 240;
        b[sec..sec + 6].copy_from_slice(b".edata");
        b[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        b[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes());
        b[opt + 112..opt + 116].copy_from_slice(&0x1000u32.to_le_bytes()); // export dir RVA
        let dir = 0x200;
        b[dir + 24..dir + 28].copy_from_slice(&(names.len() as u32).to_le_bytes());
        b[dir + 32..dir + 36].copy_from_slice(&0x1040u32.to_le_bytes()); // AddressOfNames
        let mut name_rva = 0x1080u32;
        for (i, n) in names.iter().enumerate() {
            let slot = 0x240 + i * 4;
            b[slot..slot + 4].copy_from_slice(&name_rva.to_le_bytes());
            let off = (name_rva - 0x1000 + 0x200) as usize;
            b[off..off + n.len()].copy_from_slice(n.as_bytes());
            name_rva += n.len() as u32 + 1;
        }
        b
    }

    #[test]
    fn reads_pe_export_names() {
        let pe = pe_with_exports(&["D3D11CreateDevice", "pfn_D3D11CreateDevice"]);
        assert_eq!(
            pe_export_names(&pe).unwrap(),
            ["D3D11CreateDevice", "pfn_D3D11CreateDevice"]
        );
        assert_eq!(pe_export_names(b"MZ short"), None);
        assert_eq!(pe_export_names(b"not a pe at all"), None);
    }

    #[test]
    fn recognises_axidps_dlls_of_any_age() {
        // Before the version marker: only the proxy's forwarding-table export.
        assert!(is_axidps_dll(&pe_with_exports(&["D3D11CreateDevice", PROXY_EXPORT])));
        assert!(is_axidps_dll(&pe_with_exports(&[ID_EXPORT])));
        assert!(is_axidps_dll(b"MZ axidps-dll-version=0.2.0\0"));
        // arcdps / System32 / DXVK export the D3D11 entry points only.
        assert!(!is_axidps_dll(&pe_with_exports(&[
            "D3D11CreateDevice",
            "D3D11CreateDeviceAndSwapChain",
            "get_init_addr",
        ])));
        assert!(!is_axidps_dll(b"arcdps"));
    }

    #[test]
    fn truncated_pe_is_none_not_a_panic() {
        let pe = pe_with_exports(&["AXIDPS_DLL_ID"]);
        for len in [0x40, 0x90, 0x150, 0x210, 0x250, 0x290] {
            let _ = pe_export_names(&pe[..len]);
        }
    }

    #[test]
    fn rejects_non_semver_and_unterminated() {
        assert_eq!(find_dll_version(b"axidps-dll-version=1.2\0"), None);
        assert_eq!(find_dll_version(b"axidps-dll-version=1.2.3"), None);
        assert_eq!(find_dll_version(b"axidps-dll-version=1.2.3+b.1\0").as_deref(), Some("1.2.3+b.1"));
    }
}
