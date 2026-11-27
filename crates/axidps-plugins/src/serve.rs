//! What the host serves for a plugin: safe path resolution, MIME types, the CSP and its injection.

use crate::manifest::Manifest;
use std::path::{Path, PathBuf};

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = b.get(i + 1..i + 3)
            && let Ok(h) = std::str::from_utf8(h)
            && let Ok(v) = u8::from_str_radix(h, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `rel` (percent-decoded here) as an existing file inside `root`, else `None`. `..`, absolute
/// paths, NULs and symlinks leading out all give `None`.
pub fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = percent_decode(rel)?;
    if rel.is_empty()
        || rel.contains(['\0', '\\'])
        || rel.starts_with('/')
        || rel.split('/').any(|c| c == "..")
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let file = root.join(&rel).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

/// The plugin's Content-Security-Policy, injected as a `<meta>` (see `with_csp_meta`).
pub fn csp(m: &Manifest) -> String {
    let own = format!("axidps-plugin://{}", m.id);
    let https: Vec<String> = m.network.iter().map(|n| format!("https://{n}")).collect();
    let img = if https.is_empty() {
        format!("img-src {own} data:")
    } else {
        format!("img-src {own} data: {}", https.join(" "))
    };
    let connect = if https.is_empty() {
        "connect-src 'none'".to_owned()
    } else {
        format!("connect-src {}", https.join(" "))
    };
    format!(
        "default-src 'none'; script-src {own} 'unsafe-inline'; style-src {own} 'unsafe-inline'; \
         font-src {own}; {img}; {connect}; worker-src 'none'"
    )
}

/// Whether `host` matches a `network` entry (`api.example.com`, or `*.example.com` for any
/// subdomain of it, not the bare domain), ignoring case and a trailing dot.
pub fn host_matches(entry: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let entry = entry.to_ascii_lowercase();
    match entry.strip_prefix("*.") {
        Some(base) => host.len() > base.len() + 1 && host.ends_with(&format!(".{base}")),
        None => host == entry,
    }
}

pub fn mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

/// The length of the comment `s` starts with, ended where an HTML parser ends it (the
/// tokenizer's comment states): `<!-->` and `<!--->` (abrupt), else the first `-->` or `--!>`.
/// `None` if `s` doesn't start with a comment or it never ends.
fn comment_end(s: &[u8]) -> Option<usize> {
    let body = s.strip_prefix(b"<!--")?;
    if body.starts_with(b">") {
        return Some(5);
    }
    if body.starts_with(b"->") {
        return Some(6);
    }
    (0..body.len()).find_map(|k| {
        let rest = &body[k..];
        if rest.starts_with(b"-->") {
            Some(4 + k + 3)
        } else if rest.starts_with(b"--!>") {
            Some(4 + k + 4)
        } else {
            None
        }
    })
}

/// The HTML with a CSP `<meta>` as the first element: right after a leading doctype (found past
/// a BOM, whitespace and comments), else at the very start. The parser puts it first in `<head>`
/// either way, so nothing in the plugin's file runs before the policy applies, and a doctype
/// is never displaced (which would drop the page into quirks mode).
pub fn with_csp_meta(html: Vec<u8>, csp: &str) -> Vec<u8> {
    let escaped = csp.replace('&', "&amp;").replace('"', "&quot;");
    let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{escaped}\">");
    let bom = if html.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    let mut i = bom;
    loop {
        while html.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match comment_end(&html[i..]) {
            Some(n) => i += n,
            None => break,
        }
    }
    let doctype = html.len() >= i + 9 && html[i..i + 9].eq_ignore_ascii_case(b"<!doctype");
    let at = match doctype {
        true => html[i..]
            .iter()
            .position(|&b| b == b'>')
            .map_or(bom, |p| i + p + 1),
        false => bom,
    };
    let mut out = Vec::with_capacity(html.len() + meta.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(meta.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Place, WindowDef};
    use std::fs;

    fn m(network: &[&str]) -> Manifest {
        Manifest {
            id: "example.dps-card".into(),
            name: "n".into(),
            version: "1.0.0".into(),
            author: String::new(),
            api: 1,
            permissions: vec!["fight".into()],
            network: network.iter().map(|s| s.to_string()).collect(),
            windows: vec![WindowDef {
                id: "w".into(),
                title: "t".into(),
                entry: "a.html".into(),
                description: String::new(),
                icon: "plug".into(),
                open: false,
                place: Place::BesideMeter,
                settings: Vec::new(),
            }],
        }
    }

    #[test]
    fn csp_matches_spec() {
        assert_eq!(
            csp(&m(&["api.example.com"])),
            "default-src 'none'; script-src axidps-plugin://example.dps-card 'unsafe-inline'; \
             style-src axidps-plugin://example.dps-card 'unsafe-inline'; \
             font-src axidps-plugin://example.dps-card; \
             img-src axidps-plugin://example.dps-card data: https://api.example.com; \
             connect-src https://api.example.com; worker-src 'none'"
        );
    }

    #[test]
    fn host_matching() {
        assert!(host_matches("api.example.com", "API.example.com"));
        assert!(host_matches("api.example.com", "api.example.com."));
        assert!(!host_matches("api.example.com", "x.api.example.com"));
        assert!(host_matches("*.example.com", "a.b.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(!host_matches("*.example.com", "badexample.com"));
        assert!(!host_matches("*.example.com", "example.com.evil.org"));
    }

    #[test]
    fn csp_no_network() {
        let c = csp(&m(&[]));
        assert!(c.contains("connect-src 'none'"));
        assert!(
            c.contains("img-src axidps-plugin://example.dps-card data:;"),
            "{c}"
        );
    }

    #[test]
    fn resolve_blocks_escape() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("com.example.x");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/a.js"), "1").unwrap();
        fs::write(root.join("a b.html"), "1").unwrap();
        fs::write(t.path().join("x"), "secret").unwrap();
        for bad in [
            "../x",
            "/etc/passwd",
            "a/../../x",
            "..%2fx",
            "sub",
            "",
            "a\0b",
            "..\\x",
            "%FF",
        ] {
            assert_eq!(resolve(&root, bad), None, "{bad:?}");
        }
        assert!(resolve(&root, "sub/a.js").is_some());
        assert!(resolve(&root, "a%20b.html").is_some());
        assert!(resolve(&root, "a b.html").is_some());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(t.path().join("x"), root.join("leak")).unwrap();
            std::os::unix::fs::symlink(t.path(), root.join("up")).unwrap();
            assert_eq!(resolve(&root, "leak"), None);
            assert_eq!(resolve(&root, "up/x"), None);
        }
    }

    fn inject(s: &[u8]) -> String {
        String::from_utf8(with_csp_meta(s.to_vec(), "x")).unwrap()
    }
    const META: &str = "<meta http-equiv=\"Content-Security-Policy\" content=\"x\">";

    #[test]
    fn meta_after_doctype() {
        assert_eq!(
            inject(b"<!doctype html><p>"),
            format!("<!doctype html>{META}<p>")
        );
        assert_eq!(
            inject(b"<!DOCTYPE html>\n<p>"),
            format!("<!DOCTYPE html>{META}\n<p>")
        );
    }

    #[test]
    fn meta_after_leading_whitespace_doctype() {
        assert_eq!(
            inject(b"\n  <!doctype html><p>"),
            format!("\n  <!doctype html>{META}<p>")
        );
    }

    #[test]
    fn meta_after_bom_doctype() {
        assert_eq!(
            inject(b"\xEF\xBB\xBF<!doctype html>x"),
            format!("\u{feff}<!doctype html>{META}x")
        );
        assert_eq!(inject(b"\xEF\xBB\xBF<p>"), format!("\u{feff}{META}<p>"));
    }

    #[test]
    fn meta_after_comment_then_doctype() {
        assert_eq!(
            inject(b"<!-- MIT -->\n<!-- two > -->\n<!doctype html><p>"),
            format!("<!-- MIT -->\n<!-- two > -->\n<!doctype html>{META}<p>")
        );
    }

    /// Every way an HTML parser ends a comment early: the meta must still come before anything
    /// the parser sees as markup (here a script that would hide the rest of the file from it).
    #[test]
    fn meta_not_skipped_past_early_comment_ends() {
        let evil = "<script>document.write('<plaintext hidden>')</script>";
        for (open, after) in [
            ("<!-->", "<!-- -->"),
            ("<!--->", "<!-- -->"),
            ("<!-- x --!>", "<!-- -->"),
            ("<!---->", "<!-- --!>"),
            ("<!-- a -- b --->", "<!-->"),
        ] {
            let html = format!("{open}{evil}{after}<!doctype html><p>");
            let out = inject(html.as_bytes());
            let meta = out.find(META).unwrap();
            let script = out.find("<script>").unwrap();
            assert!(meta < script, "{html:?} gave {out:?}");
            assert_eq!(out, format!("{META}{html}"), "{html:?}");
        }
        // Early-ended comments before the doctype, with nothing else: still after the doctype.
        assert_eq!(
            inject(b"<!--><!--->\n<!-- x --!><!doctype html><p>"),
            format!("<!--><!--->\n<!-- x --!><!doctype html>{META}<p>")
        );
    }

    #[test]
    fn comment_ends() {
        assert_eq!(comment_end(b"<!-->x"), Some(5));
        assert_eq!(comment_end(b"<!--->x"), Some(6));
        assert_eq!(comment_end(b"<!---->x"), Some(7));
        assert_eq!(comment_end(b"<!-- a --!>x"), Some(11));
        assert_eq!(comment_end(b"<!-- a -- > -->x"), Some(15));
        assert_eq!(comment_end(b"<!-- never"), None);
        assert_eq!(comment_end(b"<!-x -->"), None);
        assert_eq!(comment_end(b"<p>"), None);
    }

    #[test]
    fn meta_at_start_without_doctype() {
        assert_eq!(inject(b"<p>hi"), format!("{META}<p>hi"));
        assert_eq!(
            inject(b"<!-- unterminated <!doctype html>"),
            format!("{META}<!-- unterminated <!doctype html>")
        );
        assert_eq!(inject(b""), META);
        assert_eq!(inject(b"   "), format!("{META}   "));
    }

    #[test]
    fn meta_escapes_content() {
        let o = String::from_utf8(with_csp_meta(b"<p>".to_vec(), "a & \"b\"")).unwrap();
        assert!(
            o.starts_with(
                "<meta http-equiv=\"Content-Security-Policy\" content=\"a &amp; &quot;b&quot;\">"
            ),
            "{o}"
        );
    }
}
