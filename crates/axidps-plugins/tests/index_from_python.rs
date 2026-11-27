//! The registry's Python tools (darkharasho/axidps-plugins, `tools/registry`) produce what this
//! crate accepts. The fixture in `fixtures/index-from-python/` was made by its `build_index.py`
//! from one entry, a mirrored zip, and an approval, a pass and a `revoked.json` signed by its
//! `lib.sign`, with throwaway keys (`index.pub`, `approver.pub`; test-only, never release keys).

use axidps_plugins::index;
use axidps_plugins::install;
use axidps_plugins::keys::Keys;
use axidps_plugins::trust::Signed;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

const INDEX: &[u8] = include_bytes!("fixtures/index-from-python/index.json");
const SIG: &str = include_str!("fixtures/index-from-python/index.json.sig");
const ZIP: &[u8] = include_bytes!("fixtures/index-from-python/example.hello-1.0.0.zip");
const INDEX_PUB: &str = include_str!("fixtures/index-from-python/index.pub");
const APPROVER_PUB: &str = include_str!("fixtures/index-from-python/approver.pub");
const REGISTRY: &str = "https://github.com/darkharasho/axidps-plugins/releases/download/";

fn key(b64: &str) -> [u8; 32] {
    STANDARD.decode(b64.trim()).unwrap().try_into().unwrap()
}

fn keys() -> Keys {
    Keys { approvers: vec![key(APPROVER_PUB)], index: vec![key(INDEX_PUB)] }
}

#[test]
fn index_from_python() {
    let idx = index::parse(INDEX, SIG, &keys()).unwrap();
    assert!(idx.skipped.is_empty(), "{:?}", idx.skipped);
    assert_eq!(idx.generated, "2026-11-26T12:00:00Z");
    let [l] = idx.plugins.as_slice() else { panic!("{:?}", idx.plugins) };
    assert_eq!(
        (l.id.as_str(), l.name.as_str(), l.author.as_str(), l.description.as_str()),
        ("example.hello", "Hello", "someone", "Says hi.")
    );
    assert_eq!((l.icon.as_str(), l.repo.as_str()), ("chart", "someone/hello"));
    let [v] = l.versions.as_slice() else { panic!("{:?}", l.versions) };
    assert_eq!(v.version, "1.0.0");
    assert!(v.url.starts_with(REGISTRY), "{}", v.url);
    assert_eq!((v.permissions.as_slice(), v.network.as_slice(), v.api), (
        &["fight".to_string(), "world".to_string()][..],
        &["api.example.com".to_string()][..],
        1
    ));

    // Every signed item verifies and parses whole.
    let (signed, log) = Signed::from_index(&idx.raw, &keys(), 0);
    assert!(log.is_empty(), "{log:?}");
    let [a] = signed.approvals.as_slice() else { panic!("{:?}", signed.approvals) };
    assert_eq!((a.tree.as_str(), a.verified, a.grant.as_slice()), (v.tree.as_str(), true, &["world".to_string()][..]));
    let [p] = signed.passes.as_slice() else { panic!("{:?}", signed.passes) };
    assert_eq!((p.login.as_str(), p.expires.as_str()), ("someone", "2027-02-24"));
    assert_eq!(p.ids, ["example.*"]);
    let r = signed.revoked.as_ref().unwrap();
    assert_eq!(r.seq, 1);
    assert_eq!(r.verifications, [("example.other".into(), "0.1.0".into(), "test".into())]);
    assert!(l.approved(v, &signed));
    assert_eq!(l.offer(&signed).unwrap().version, "1.0.0");

    // The index key alone vouches for no approval, pass or revocation.
    let index_only = Keys { approvers: vec![key(INDEX_PUB)], index: vec![key(INDEX_PUB)] };
    let (s, log) = Signed::from_index(&idx.raw, &index_only, 0);
    assert!(s.approvals.is_empty() && s.passes.is_empty() && s.revoked.is_none());
    assert_eq!(log.len(), 3, "{log:?}");

    // The approver key can't sign the index.
    let swapped = Keys { approvers: vec![], index: vec![key(APPROVER_PUB)] };
    assert!(index::parse(INDEX, SIG, &swapped).is_err());
}

#[test]
fn index_from_python_package_installs() {
    // The zip's SHA-256 and the tree hash Python computed are what the installer gets.
    let idx = index::parse(INDEX, SIG, &keys()).unwrap();
    let v = &idx.plugins[0].versions[0];
    let t = tempfile::tempdir().unwrap();
    let (plugins, store) = (t.path().join("plugins"), t.path().join("store"));
    install::install(&plugins, &store, "example.hello", v, ZIP).unwrap();
    let rec = install::Record::load(&store);
    assert_eq!(rec.plugins["example.hello"].tree, v.tree);
    assert!(rec.owns("example.hello", &plugins.join("example.hello")));
}
