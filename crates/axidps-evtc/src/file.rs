//! Log file naming and atomic writing.

use std::io;
use std::path::{Path, PathBuf};

/// `YYYYMMDD-HHMMSS` from the local fight-end time.
pub fn log_file_stem(end_local: chrono::NaiveDateTime) -> String {
    end_local.format("%Y%m%d-%H%M%S").to_string()
}

/// `<documents>/Guild Wars 2/addons/arcdps/arcdps.cbtlogs`.
pub fn default_log_dir(documents: &Path) -> PathBuf {
    documents
        .join("Guild Wars 2")
        .join("addons")
        .join("arcdps")
        .join("arcdps.cbtlogs")
}

/// Write `<stem>.zevtc` into `dir` atomically (temp file, sync, rename),
/// never overwriting: a taken name gets `_2`, `_3`, ... Returns the final path.
pub fn write_log_atomic(dir: &Path, stem: &str, evtc: &[u8]) -> io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut final_stem = stem.to_string();
    let mut n = 1u32;
    while dir.join(format!("{final_stem}.zevtc")).exists() {
        n += 1;
        final_stem = format!("{stem}_{n}");
    }
    let dest = dir.join(format!("{final_stem}.zevtc"));
    // Watchers only react to files created or renamed into place, and must
    // never see a partial file: write under a non-.zevtc name, then rename.
    let tmp = dir.join(format!("{final_stem}.zevtc.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    let res = f
        .write_all(&crate::zevtc(&final_stem, evtc))
        .and_then(|()| f.sync_all());
    drop(f);
    match res.and_then(|()| std::fs::rename(&tmp, &dest)) {
        Ok(()) => Ok(dest),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unzevtc;

    #[test]
    fn stem_format() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 11, 18)
            .unwrap()
            .and_hms_opt(20, 6, 1)
            .unwrap();
        assert_eq!(log_file_stem(t), "20261118-200601");
    }

    #[test]
    fn default_dir() {
        assert_eq!(
            default_log_dir(Path::new("/d")),
            Path::new("/d/Guild Wars 2/addons/arcdps/arcdps.cbtlogs")
        );
    }

    #[test]
    fn atomic_write_creates_dir_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b");
        let p = write_log_atomic(&dir, "20261118-200601", b"EVTC").unwrap();
        assert_eq!(p, dir.join("20261118-200601.zevtc"));
        assert!(p.exists());
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["20261118-200601.zevtc"]);
        assert_eq!(unzevtc(&std::fs::read(&p).unwrap()).unwrap(), b"EVTC");
    }

    #[test]
    fn atomic_write_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = write_log_atomic(tmp.path(), "20261118-200601", b"EVTC1").unwrap();
        let p2 = write_log_atomic(tmp.path(), "20261118-200601", b"EVTC2").unwrap();
        assert_eq!(p1, tmp.path().join("20261118-200601.zevtc"));
        assert_eq!(p2, tmp.path().join("20261118-200601_2.zevtc"));
        assert_eq!(unzevtc(&std::fs::read(&p1).unwrap()).unwrap(), b"EVTC1");
        assert_eq!(unzevtc(&std::fs::read(&p2).unwrap()).unwrap(), b"EVTC2");
    }
}
