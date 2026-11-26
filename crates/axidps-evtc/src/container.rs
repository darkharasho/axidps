//! `.zevtc` container: a zip with a single deflated entry.

use std::io;

/// Wrap `evtc` in a zip with one deflated entry named `entry_name`.
pub fn zevtc(entry_name: &str, evtc: &[u8]) -> Vec<u8> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    // A seekable writer lets the zip crate patch sizes and CRC into the local
    // header (no data-descriptor flag), which single-pass readers rely on.
    let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    w.start_file(entry_name, opts)
        .expect("zip start_file to memory");
    w.write_all(evtc).expect("zip write to memory");
    w.finish().expect("zip finish to memory").into_inner()
}

/// Return the bytes of the first entry of a `.zevtc`.
pub fn unzevtc(bytes: &[u8]) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut ar = zip::ZipArchive::new(io::Cursor::new(bytes)).map_err(io::Error::other)?;
    let mut f = ar.by_index(0).map_err(io::Error::other)?;
    let mut out = Vec::new();
    f.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zevtc_roundtrip() {
        let z = zevtc("20261118-200601", b"EVTCabc");
        assert_eq!(unzevtc(&z).unwrap(), b"EVTCabc");
        assert_eq!(&z[0..4], b"PK\x03\x04");
        assert_eq!(u16::from_le_bytes([z[8], z[9]]), 8, "deflate");
        let namelen = u16::from_le_bytes([z[26], z[27]]) as usize;
        assert_eq!(&z[30..30 + namelen], b"20261118-200601");
    }

    /// The local header must carry the real compressed size (no data
    /// descriptor), so single-pass readers like axilog's can slice the data.
    #[test]
    fn local_header_has_sizes_and_no_descriptor_flag() {
        use std::io::Read;
        let payload: Vec<u8> = (0..5000u32).map(|i| (i % 7) as u8).collect();
        let z = zevtc("x", &payload);
        let flags = u16::from_le_bytes([z[6], z[7]]);
        assert_eq!(flags & 0x8, 0, "data descriptor flag set");
        let csize = u32::from_le_bytes(z[18..22].try_into().unwrap()) as usize;
        let usize_ = u32::from_le_bytes(z[22..26].try_into().unwrap()) as usize;
        assert!(csize > 0);
        assert_eq!(usize_, payload.len());
        let namelen = u16::from_le_bytes([z[26], z[27]]) as usize;
        let extralen = u16::from_le_bytes([z[28], z[29]]) as usize;
        let start = 30 + namelen + extralen;
        let mut out = Vec::new();
        flate2::read::DeflateDecoder::new(&z[start..start + csize])
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, payload);
    }
}
