//! Store trust: approver-signed approvals, developer passes and revocations, and how far each
//! scanned plugin is trusted. No clock reads here: `Ctx::today` is passed in.

use crate::keys::Keys;
use crate::logline::clean;
use crate::scan::Plugin;
use crate::sign::verify_object;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value};
use std::collections::HashSet;

/// An approval for one exact version and tree hash of a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub id: String,
    pub version: String,
    pub tree: String,
    pub verified: bool,
    pub grant: Vec<String>,
}

/// A developer pass: deeper permissions for the holder of `key`'s private half, on the
/// author's own plugins (`ids`, exact or `prefix.*`), until `expires` (`YYYY-MM-DD`, UTC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pass {
    pub login: String,
    pub ids: Vec<String>,
    pub grant: Vec<String>,
    pub expires: String,
    pub key: [u8; 32],
}

/// `revoked.json`: what no longer loads or no longer counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Revoked {
    pub seq: u64,
    /// (id, version, tree, reason): that content stops loading.
    pub versions: Vec<(String, String, String, String)>,
    /// (id, version, reason): that version loses Verified and any approval grant.
    pub verifications: Vec<(String, String, String)>,
    /// (id, reason): every version stops loading.
    pub ids: Vec<(String, String)>,
    /// (login, key, reason): that pass no longer counts.
    pub passes: Vec<(String, [u8; 32], String)>,
}

/// The approver-signed items of an index, each checked on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signed {
    pub approvals: Vec<Approval>,
    pub passes: Vec<Pass>,
    pub revoked: Option<Revoked>,
}

impl Signed {
    /// The approvals, passes and revocation in index `v` that verify against `keys.approvers`.
    /// A bad item is skipped with a log line naming it; a `revoked` older than `min_seq` is
    /// ignored (an old copy can't undo a revocation), and when `min_seq > 0` and no `revoked`
    /// at least that new is accepted, the approvals and passes are dropped too.
    pub fn from_index(v: &Value, keys: &Keys, min_seq: u64) -> (Signed, Vec<String>) {
        let mut s = Signed::default();
        let mut log = Vec::new();
        let k = &keys.approvers;
        for item in items(v, "approvals", &mut log) {
            match checked(item, "approval", k, parse_approval) {
                Ok(a) => s.approvals.push(a),
                Err(e) => log.push(refused(item, "approval", &["id", "version"], &e)),
            }
        }
        for item in items(v, "passes", &mut log) {
            match checked(item, "pass", k, parse_pass) {
                Ok(p) => s.passes.push(p),
                Err(e) => log.push(refused(item, "pass", &["login"], &e)),
            }
        }
        if let Some(r) = v.get("revoked").filter(|r| !r.is_null()) {
            match checked(r, "revoked", k, parse_revoked) {
                Ok(r) if r.seq < min_seq => log.push(format!(
                    "revoked.json seq {} ignored: seq {min_seq} was already seen",
                    r.seq
                )),
                Ok(r) => s.revoked = Some(r),
                Err(e) => log.push(clean(&format!("revoked.json refused: {e}"))),
            }
        }
        // Fail closed: an index without a revocation at least as new as the last one seen
        // (missing, stale or refused) could be an old copy from before something was revoked,
        // so its approvals and passes unlock nothing.
        if min_seq > 0 && s.revoked.is_none() {
            log.push(format!(
                "index is older than the last revocation seen (seq {min_seq}): \
                 its approvals and passes are ignored"
            ));
            s.approvals.clear();
            s.passes.clear();
        }
        (s, log)
    }
}

/// The array `v[name]`; missing means none, anything else is logged and means none.
fn items<'a>(v: &'a Value, name: &str, log: &mut Vec<String>) -> &'a [Value] {
    match v.get(name) {
        None | Some(Value::Null) => &[],
        Some(Value::Array(a)) => a,
        Some(_) => {
            log.push(format!("index {name} refused: not a list"));
            &[]
        }
    }
}

/// The signature first, then the fields: an item is used whole or not at all.
fn checked<T>(
    v: &Value,
    kind: &str,
    keys: &[[u8; 32]],
    parse: fn(&Map<String, Value>) -> Result<T, String>,
) -> Result<T, String> {
    verify_object(v, kind, keys)?;
    parse(v.as_object().ok_or("not an object")?)
}

/// "`kind` `label…` refused: `why`", the label taken from the (unverified) item's fields.
fn refused(item: &Value, kind: &str, label: &[&str], why: &str) -> String {
    let label: Vec<&str> = label
        .iter()
        .map(|f| item.get(f).and_then(Value::as_str).unwrap_or("?"))
        .collect();
    clean(&format!("{kind} {} refused: {why}", label.join(" ")))
}

fn str_of(o: &Map<String, Value>, f: &str) -> Result<String, String> {
    o.get(f)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{f}: not a string"))
}

fn list_of(o: &Map<String, Value>, f: &str) -> Result<Vec<String>, String> {
    o.get(f)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{f}: not a list"))?
        .iter()
        .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| format!("{f}: not a list of strings")))
        .collect()
}

/// Entries of the optional list `f`, each an object parsed by `each`.
fn entries<T>(
    o: &Map<String, Value>,
    f: &str,
    each: impl Fn(&Map<String, Value>) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let Some(v) = o.get(f) else { return Ok(Vec::new()) };
    v.as_array()
        .ok_or_else(|| format!("{f}: not a list"))?
        .iter()
        .map(|e| each(e.as_object().ok_or_else(|| format!("{f}: not a list of objects"))?))
        .collect()
}

fn key_of(o: &Map<String, Value>, f: &str) -> Result<[u8; 32], String> {
    STANDARD
        .decode(str_of(o, f)?)
        .ok()
        .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
        .ok_or_else(|| format!("{f}: not a base64 32-byte key"))
}

/// `YYYY-MM-DD`, digits only, so dates compare correctly as strings.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

fn parse_approval(o: &Map<String, Value>) -> Result<Approval, String> {
    Ok(Approval {
        id: str_of(o, "id")?,
        version: str_of(o, "version")?,
        tree: str_of(o, "tree")?,
        verified: o.get("verified").and_then(Value::as_bool).ok_or("verified: not a bool")?,
        grant: list_of(o, "grant")?,
    })
}

fn parse_pass(o: &Map<String, Value>) -> Result<Pass, String> {
    let expires = str_of(o, "expires")?;
    if !is_date(&expires) {
        return Err("expires: not YYYY-MM-DD".into());
    }
    Ok(Pass {
        login: str_of(o, "login")?,
        ids: list_of(o, "ids")?,
        grant: list_of(o, "grant")?,
        expires,
        key: key_of(o, "key")?,
    })
}

fn parse_revoked(o: &Map<String, Value>) -> Result<Revoked, String> {
    Ok(Revoked {
        seq: o.get("seq").and_then(Value::as_u64).ok_or("seq: not a number")?,
        versions: entries(o, "versions", |e| {
            Ok((str_of(e, "id")?, str_of(e, "version")?, str_of(e, "tree")?, str_of(e, "reason")?))
        })?,
        verifications: entries(o, "verifications", |e| {
            Ok((str_of(e, "id")?, str_of(e, "version")?, str_of(e, "reason")?))
        })?,
        ids: entries(o, "ids", |e| Ok((str_of(e, "id")?, str_of(e, "reason")?)))?,
        passes: entries(o, "passes", |e| {
            Ok((str_of(e, "login")?, key_of(e, "key")?, str_of(e, "reason")?))
        })?,
    })
}

/// What the trust decision needs besides the plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ctx {
    pub signed: Signed,
    /// Developer mode on.
    pub dev: bool,
    /// The public half of `<config>/dev-key`.
    pub dev_key: Option<[u8; 32]>,
    /// Today, UTC, `YYYY-MM-DD`.
    pub today: String,
    /// Ids of the plugins the store installed.
    pub store_ids: HashSet<String>,
}

impl Ctx {
    /// No index, no developer mode: player plugins are Unverified, bundled ones Bundled.
    pub fn none() -> Ctx {
        Ctx::default()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Trust {
    Bundled,
    Verified,
    #[default]
    Unverified,
    Dev,
    Revoked,
}

impl Trust {
    /// The page's name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Bundled => "bundled",
            Trust::Verified => "verified",
            Trust::Unverified => "unverified",
            Trust::Dev => "dev",
            Trust::Revoked => "revoked",
        }
    }
}

/// How far a plugin is trusted: its level, the permissions that level grants (deeper ones
/// load only if listed here), and why, when something was revoked or couldn't be checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    pub level: Trust,
    pub granted: Vec<String>,
    pub reason: Option<String>,
}

/// The trust decision for `p`, whose files hash to `tree` (lowercase hex; `None` if they
/// couldn't be hashed).
pub fn decide(p: &Plugin, tree: Option<&str>, ctx: &Ctx) -> Decision {
    let decision = |level, granted, reason| Decision { level, granted, reason };
    let Some(m) = &p.manifest else {
        let level = if p.bundled { Trust::Bundled } else { Trust::Unverified };
        return decision(level, Vec::new(), None);
    };
    if p.bundled {
        return decision(Trust::Bundled, m.permissions.clone(), None);
    }
    let revoked = ctx.signed.revoked.as_ref();
    let pulled = revoked.and_then(|r| {
        let by_id = r.ids.iter().find(|(id, _)| *id == m.id).map(|(_, why)| why);
        by_id.or_else(|| {
            r.versions
                .iter()
                .find(|(id, v, t, _)| *id == m.id && *v == m.version && Some(t.as_str()) == tree)
                .map(|(.., why)| why)
        })
    });
    if let Some(why) = pulled {
        return decision(Trust::Revoked, Vec::new(), Some(why.clone()));
    }
    let Some(tree) = tree else {
        let why = "its files couldn't be hashed, so it can't be verified";
        return decision(Trust::Unverified, Vec::new(), Some(why.into()));
    };
    let deeper = m.deeper();
    let within = |grants: &[&Vec<String>]| -> Vec<String> {
        deeper.iter().filter(|d| grants.iter().any(|g| g.contains(d))).cloned().collect()
    };
    let mut reason = None;

    let approvals: Vec<&Vec<String>> = ctx
        .signed
        .approvals
        .iter()
        .filter(|a| a.verified && a.id == m.id && a.version == m.version && a.tree == tree)
        .map(|a| &a.grant)
        .collect();
    if !approvals.is_empty() {
        let lost = revoked.and_then(|r| {
            r.verifications.iter().find(|(id, v, _)| *id == m.id && *v == m.version)
        });
        match lost {
            None => return decision(Trust::Verified, within(&approvals), None),
            Some((.., why)) => reason = Some(format!("Verification revoked: {why}")),
        }
    }

    let store = p.store || ctx.store_ids.contains(&m.id);
    if let (true, false, Some(key), true) = (ctx.dev, store, ctx.dev_key, is_date(&ctx.today)) {
        let mut grants = Vec::new();
        for pass in &ctx.signed.passes {
            if pass.key != key || !pass.ids.iter().any(|pat| id_matches(pat, &m.id)) {
                continue;
            }
            let pulled = revoked.and_then(|r| {
                r.passes.iter().find(|(login, k, _)| *login == pass.login && *k == pass.key)
            });
            if let Some((.., why)) = pulled {
                reason = Some(format!("Developer pass revoked: {why}"));
            } else if ctx.today > pass.expires {
                reason = Some(format!("Developer pass expired on {}", pass.expires));
            } else {
                grants.push(&pass.grant);
            }
        }
        if !grants.is_empty() {
            return decision(Trust::Dev, within(&grants), None);
        }
    }
    decision(Trust::Unverified, Vec::new(), reason)
}

/// A pass `ids` entry: an exact id, or `prefix.*` for any id under `prefix.`.
fn id_matches(pattern: &str, id: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => id
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|rest| !rest.is_empty()),
        None => pattern == id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::loadable;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    const TREE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_TREE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TODAY: &str = "2026-11-26";

    fn approver() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn keys() -> Keys {
        Keys { approvers: vec![approver().verifying_key().to_bytes()], index: vec![] }
    }

    fn dev_pk() -> [u8; 32] {
        SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes()
    }

    /// `body` with `approver` and a `sig` by the test approver key.
    fn sign(kind: &str, mut body: Value) -> Value {
        let k = approver();
        body["approver"] = json!(crate::sign::key_id(&k.verifying_key().to_bytes()));
        let msg = format!("axidps-{kind}-v1\n{}", crate::sign::canonical(&body).unwrap());
        body["sig"] = json!(STANDARD.encode(k.sign(msg.as_bytes()).to_bytes()));
        body
    }

    fn approval(id: &str, tree: &str, grant: &[&str]) -> Value {
        sign(
            "approval",
            json!({"id": id, "version": "1.0.0", "tree": tree, "verified": true,
                   "grant": grant, "signed": "2026-11-26"}),
        )
    }

    fn pass(ids: &[&str], grant: &[&str], expires: &str) -> Value {
        sign(
            "pass",
            json!({"login": "someone", "ids": ids, "grant": grant, "expires": expires,
                   "key": STANDARD.encode(dev_pk())}),
        )
    }

    fn revoked(seq: u64, extra: Value) -> Value {
        let mut body = json!({"seq": seq, "versions": [], "verifications": [], "ids": [],
                              "passes": []});
        for (k, v) in extra.as_object().unwrap() {
            body[k.as_str()] = v.clone();
        }
        sign("revoked", body)
    }

    fn signed(index: Value) -> Signed {
        let (s, log) = Signed::from_index(&index, &keys(), 0);
        assert!(log.is_empty(), "{log:?}");
        s
    }

    fn plugin(id: &str, bundled: bool, permissions: &[&str]) -> Plugin {
        let m = format!(
            r#"{{"id":"{id}","name":"n","version":"1.0.0","api":1,"permissions":{},"windows":[{{"id":"w","title":"t","entry":"a.html"}}]}}"#,
            json!(permissions)
        );
        let m = crate::manifest::problems(m.as_bytes(), Path::new("/nonexistent-axidps-test"))
            .unwrap();
        Plugin {
            dir: PathBuf::from("/x"),
            bundled,
            folder: id.into(),
            manifest: Some(m),
            error: None,
            trust: Decision::default(),
            store: false,
        }
    }

    /// `p` with the decision for `tree` under `ctx`, as the scan would set it.
    fn decided(mut p: Plugin, tree: Option<&str>, ctx: &Ctx) -> Plugin {
        p.trust = decide(&p, tree, ctx);
        p
    }

    fn ctx(signed: Signed) -> Ctx {
        Ctx { signed, today: TODAY.into(), ..Ctx::none() }
    }

    fn dev_ctx(signed: Signed) -> Ctx {
        Ctx { dev: true, dev_key: Some(dev_pk()), ..ctx(signed) }
    }

    #[test]
    fn approval_verifies_and_grants_intersection() {
        let s = signed(json!({"approvals": [approval("example.x", TREE, &["textures"])]}));
        let p = decided(plugin("example.x", false, &["fight", "textures", "world"]), Some(TREE), &ctx(s.clone()));
        assert_eq!(p.trust.level, Trust::Verified);
        assert_eq!(p.trust.granted, ["textures"]);
        assert!(!loadable(&p), "world isn't granted");

        let p = decided(plugin("example.x", false, &["fight", "textures"]), Some(TREE), &ctx(s));
        assert_eq!(p.trust.level, Trust::Verified);
        assert!(loadable(&p));
    }

    #[test]
    fn edited_file_is_unverified() {
        let s = signed(json!({"approvals": [approval("example.x", TREE, &["textures"])]}));
        let p = decided(plugin("example.x", false, &["fight", "textures"]), Some(OTHER_TREE), &ctx(s.clone()));
        assert_eq!(p.trust.level, Trust::Unverified);
        assert!(p.trust.granted.is_empty());
        assert!(!loadable(&p));
        // Files that couldn't be hashed can't be verified either, and say why.
        let p = decided(plugin("example.x", false, &["fight", "textures"]), None, &ctx(s));
        assert_eq!(p.trust.level, Trust::Unverified);
        assert!(p.trust.reason.is_some());
        assert!(!loadable(&p));
        // A basic plugin still loads unverified.
        let p = decided(plugin("example.y", false, &["fight"]), Some(OTHER_TREE), &ctx(Signed::default()));
        assert_eq!(p.trust.level, Trust::Unverified);
        assert!(loadable(&p));
    }

    #[test]
    fn tampered_fields_are_refused() {
        let good_a = approval("example.x", TREE, &["textures"]);
        let mut wide = good_a.clone();
        wide["grant"] = json!(["textures", "world"]);
        let mut swapped = good_a.clone();
        swapped["tree"] = json!(OTHER_TREE);
        let good_p = pass(&["example.*"], &["world"], "2026-12-01");
        let mut later = good_p.clone();
        later["expires"] = json!("2099-12-01");
        let mut rv = revoked(3, json!({"ids": [{"id": "example.x", "reason": "bad"}]}));
        rv["ids"] = json!([]);
        let index = json!({
            "approvals": [wide, good_a, swapped, "junk"],
            "passes": [later, good_p],
            "revoked": rv,
        });
        let (s, log) = Signed::from_index(&index, &keys(), 0);
        assert_eq!(s.approvals.len(), 1, "{log:?}");
        assert_eq!(s.approvals[0].grant, ["textures"]);
        assert_eq!(s.approvals[0].tree, TREE);
        assert_eq!(s.passes.len(), 1);
        assert_eq!(s.passes[0].expires, "2026-12-01");
        assert!(s.revoked.is_none());
        assert_eq!(log.len(), 5, "{log:?}");
        assert_eq!(log.iter().filter(|l| l.contains("approval example.x 1.0.0")).count(), 2, "{log:?}");
        assert!(log.iter().any(|l| l.contains("approval")), "{log:?}");
        assert!(log.iter().any(|l| l.contains("pass someone")), "{log:?}");
        assert!(log.iter().any(|l| l.contains("revoked.json")), "{log:?}");

        // Signed, but malformed: refused whole, not read loosely.
        let bad_key = sign("pass", json!({"login": "someone", "ids": ["example.*"], "grant": [],
                                          "expires": "2026-12-01", "key": "AAAA"}));
        let bad_date = pass(&["example.*"], &[], "soon");
        let no_tree = sign("approval", json!({"id": "example.x", "version": "1.0.0",
                                              "verified": true, "grant": []}));
        let (s, log) = Signed::from_index(
            &json!({"approvals": [no_tree], "passes": [bad_key, bad_date]}),
            &keys(),
            0,
        );
        assert_eq!(s, Signed::default());
        assert_eq!(log.len(), 3, "{log:?}");

        // Signed by a key this build doesn't trust.
        let other = Keys { approvers: vec![dev_pk()], index: vec![] };
        let (s, log) = Signed::from_index(&json!({"approvals": [approval("example.x", TREE, &[])]}), &other, 0);
        assert!(s.approvals.is_empty());
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn pass_rules() {
        let ok = || signed(json!({"passes": [pass(&["someone.*", "example.exact"], &["world", "squad"], "2026-12-01")]}));
        let perms = &["fight", "world", "textures"][..];
        let good = || plugin("someone.x", false, perms);

        let p = decided(good(), Some(TREE), &dev_ctx(ok()));
        assert_eq!(p.trust.level, Trust::Dev);
        assert_eq!(p.trust.granted, ["world"]);
        assert!(!loadable(&p), "textures isn't in the pass");
        let p = decided(plugin("someone.x", false, &["world"]), Some(TREE), &dev_ctx(ok()));
        assert!(loadable(&p));
        let p = decided(plugin("example.exact", false, perms), Some(TREE), &dev_ctx(ok()));
        assert_eq!(p.trust.level, Trust::Dev);
        // Expiry day itself still counts.
        let on_day = Ctx { today: "2026-12-01".into(), ..dev_ctx(ok()) };
        assert_eq!(decide(&good(), Some(TREE), &on_day).level, Trust::Dev);

        let mut store = dev_ctx(ok());
        store.store_ids.insert("someone.x".into());
        let mut store_flag = good();
        store_flag.store = true;
        let revoked_pass = {
            let mut s = ok();
            let mut i = signed(json!({"revoked": revoked(1, json!({"passes": [
                {"login": "someone", "key": STANDARD.encode(dev_pk()), "reason": "leaked"}]}))}));
            s.revoked = i.revoked.take();
            s
        };
        let cases: Vec<(&str, Plugin, Option<&str>, Ctx)> = vec![
            ("dev off", good(), Some(TREE), Ctx { dev: false, ..dev_ctx(ok()) }),
            ("expired", good(), Some(TREE), Ctx { today: "2026-12-02".into(), ..dev_ctx(ok()) }),
            ("no today", good(), Some(TREE), Ctx { today: String::new(), ..dev_ctx(ok()) }),
            ("no dev key", good(), Some(TREE), Ctx { dev_key: None, ..dev_ctx(ok()) }),
            ("dev key mismatch", good(), Some(TREE), Ctx { dev_key: Some([1; 32]), ..dev_ctx(ok()) }),
            ("store install", good(), Some(TREE), store),
            ("store flag", store_flag, Some(TREE), dev_ctx(ok())),
            ("id not in ids", plugin("stranger.x", false, perms), Some(TREE), dev_ctx(ok())),
            ("prefix only", plugin("someoneelse.x", false, perms), Some(TREE), dev_ctx(ok())),
            ("exact only", plugin("example.exact2", false, perms), Some(TREE), dev_ctx(ok())),
            ("no tree", good(), None, dev_ctx(ok())),
            ("revoked pass", good(), Some(TREE), dev_ctx(revoked_pass)),
        ];
        for (name, p, tree, c) in cases {
            let p = decided(p, tree, &c);
            assert_eq!(p.trust.level, Trust::Unverified, "{name}");
            assert!(p.trust.granted.is_empty(), "{name}");
            assert!(!loadable(&p), "{name}");
        }
    }

    #[test]
    fn revocations() {
        let rv = |extra: Value| {
            signed(json!({
                "approvals": [approval("example.x", TREE, &["world"])],
                "revoked": revoked(2, extra),
            }))
        };
        let perms = &["fight", "world"][..];

        let s = rv(json!({"versions": [{"id": "example.x", "version": "1.0.0", "tree": TREE, "reason": "malware"}]}));
        let p = decided(plugin("example.x", false, perms), Some(TREE), &ctx(s.clone()));
        assert_eq!(p.trust.level, Trust::Revoked);
        assert_eq!(p.trust.reason.as_deref(), Some("malware"));
        assert!(!loadable(&p));
        let basic = decided(plugin("example.x", false, &["fight"]), Some(TREE), &ctx(s.clone()));
        assert!(!loadable(&basic), "a revoked version never loads");
        // Other content of that version isn't this revocation.
        let p = decided(plugin("example.x", false, &["fight"]), Some(OTHER_TREE), &ctx(s));
        assert_eq!(p.trust.level, Trust::Unverified);
        assert!(loadable(&p));

        let s = rv(json!({"verifications": [{"id": "example.x", "version": "1.0.0", "reason": "review missed it"}]}));
        let p = decided(plugin("example.x", false, perms), Some(TREE), &ctx(s.clone()));
        assert_eq!(p.trust.level, Trust::Unverified);
        assert!(p.trust.granted.is_empty());
        assert!(p.trust.reason.as_deref().unwrap().contains("review missed it"));
        assert!(!loadable(&p));
        let basic = decided(plugin("example.x", false, &["fight"]), Some(TREE), &ctx(s));
        assert!(loadable(&basic), "still loads with basic permissions");

        let s = rv(json!({"ids": [{"id": "example.x", "reason": "gone"}]}));
        for tree in [Some(TREE), Some(OTHER_TREE), None] {
            let p = decided(plugin("example.x", false, &["fight"]), tree, &ctx(s.clone()));
            assert_eq!(p.trust.level, Trust::Revoked);
            assert_eq!(p.trust.reason.as_deref(), Some("gone"));
            assert!(!loadable(&p));
        }
    }

    #[test]
    fn revocation_seq_never_goes_back() {
        let index = json!({"revoked": revoked(4, json!({"ids": []}))});
        let (s, log) = Signed::from_index(&index, &keys(), 5);
        assert!(s.revoked.is_none());
        assert!(log[0].contains("revoked.json"), "{log:?}");
        let (s, log) = Signed::from_index(&index, &keys(), 4);
        assert_eq!(s.revoked.unwrap().seq, 4);
        assert!(log.is_empty());
    }

    #[test]
    fn index_older_than_the_last_revocation_unlocks_nothing() {
        let a = approval("example.x", TREE, &["world"]);
        let p = pass(&["example.*"], &["world"], "2026-12-01");
        let perms = &["fight", "world"][..];
        let stale = json!({"approvals": [a.clone()], "passes": [p.clone()],
                           "revoked": revoked(4, json!({}))});
        let bare = json!({"approvals": [a.clone()], "passes": [p.clone()]});
        let mut forged = revoked(5, json!({}));
        forged["seq"] = json!(9);
        let refused = json!({"approvals": [a.clone()], "passes": [p.clone()], "revoked": forged});
        for (name, index) in [("stale", &stale), ("no revoked", &bare), ("refused", &refused)] {
            let (s, log) = Signed::from_index(index, &keys(), 5);
            assert_eq!(s, Signed::default(), "{name}");
            assert!(log.iter().any(|l| l.contains("older than the last revocation")), "{name}: {log:?}");
            let d = decide(&plugin("example.x", false, perms), Some(TREE), &dev_ctx(s));
            assert_eq!(d.level, Trust::Unverified, "{name}");
            assert!(d.granted.is_empty(), "{name}");
        }
        // Current revocation: everything counts.
        let current = json!({"approvals": [a.clone()], "passes": [p.clone()],
                             "revoked": revoked(5, json!({}))});
        let (s, log) = Signed::from_index(&current, &keys(), 5);
        assert!(log.is_empty(), "{log:?}");
        assert_eq!((s.approvals.len(), s.passes.len(), s.revoked.as_ref().map(|r| r.seq)), (1, 1, Some(5)));
        // Nothing seen yet: an index without revoked still counts.
        let (s, log) = Signed::from_index(&bare, &keys(), 0);
        assert!(log.is_empty(), "{log:?}");
        let d = decide(&plugin("example.x", false, perms), Some(TREE), &ctx(s));
        assert_eq!(d.level, Trust::Verified);
        assert_eq!(d.granted, ["world"]);
    }

    #[test]
    fn bundled_needs_no_approval() {
        let p = decided(plugin("axidps.thing", true, &["fight", "world"]), None, &Ctx::none());
        assert_eq!(p.trust.level, Trust::Bundled);
        assert_eq!(p.trust.granted, ["fight", "world"]);
        assert!(loadable(&p));
    }
}
