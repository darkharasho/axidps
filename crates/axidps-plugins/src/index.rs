//! The store index: `index.json`, signed by an index key over `"axidps-index-v1\n" + bytes`.
//! The index key vouches for the listing only; approvals inside it are checked separately
//! (`trust::Signed::from_index` over `Index::raw`) and alone can grant more than basic.

use crate::keys::Keys;
use crate::logline::clean;
use crate::manifest::valid_id;
use crate::sign::verify_bytes;
use crate::manifest::{Level, level};
use crate::trust::{Signed, Trust};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// The prefix the index signature covers.
pub const INDEX_PREFIX: &str = "axidps-index-v1\n";

/// One published version of a listed plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub version: String,
    /// Where the package zip downloads from (`https://` only).
    pub url: String,
    /// SHA-256 of the zip, lowercase hex.
    pub sha256: String,
    /// Tree hash of the unpacked files, lowercase hex.
    pub tree: String,
    pub permissions: Vec<String>,
    pub network: Vec<String>,
    pub api: u32,
}

impl Signed {
    /// Fills `covered` with the versions in `plugins` that a standing approval clears.
    pub fn cover(&mut self, plugins: &[Listing]) {
        let mut covered = HashSet::new();
        for l in plugins {
            for v in &l.versions {
                if l.cleared(v, self) == Some(Trust::Approved) {
                    covered.insert((l.id.clone(), v.version.clone(), v.tree.clone()));
                }
            }
        }
        self.covered = covered;
    }
}

/// A plugin in the index, its versions oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub icon: String,
    pub repo: String,
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    pub generated: String,
    pub plugins: Vec<Listing>,
    /// The whole parsed index, for `trust::Signed::from_index`.
    pub raw: Value,
    /// One line per listing or version that was skipped, and why.
    pub skipped: Vec<String>,
}

/// `index.json` checked against `sig_b64` (the contents of `index.json.sig`) and the index
/// keys, then parsed. The signature is checked before anything is read. Listings and versions
/// that don't parse are skipped with a line in `skipped`, as are `axidps.*` ids (reserved for
/// bundled plugins) and repeated ids or versions.
pub fn parse(bytes: &[u8], sig_b64: &str, keys: &Keys) -> Result<Index, String> {
    if !verify_bytes(INDEX_PREFIX, bytes, sig_b64.trim(), &keys.index) {
        return Err("index signature doesn't verify".into());
    }
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("index: not valid JSON ({e})"))?;
    let o = raw.as_object().ok_or("index: not a JSON object")?;
    let generated = o.get("generated").and_then(Value::as_str).unwrap_or("").to_owned();
    let list = match o.get("plugins") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(a)) => a,
        Some(_) => return Err("index: plugins is not a list".into()),
    };
    let mut plugins = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();
    for item in list {
        let label = item.get("id").and_then(Value::as_str).unwrap_or("?");
        match listing(item, &mut skipped) {
            Err(e) => skipped.push(clean(&format!("listing {label} skipped: {e}"))),
            Ok(l) if !seen.insert(l.id.clone()) => {
                skipped.push(clean(&format!("listing {label} skipped: listed twice")))
            }
            Ok(l) => plugins.push(l),
        }
    }
    Ok(Index { generated, plugins, raw, skipped })
}

fn listing(v: &Value, skipped: &mut Vec<String>) -> Result<Listing, String> {
    let o = v.as_object().ok_or("not an object")?;
    let id = str_of(o, "id")?;
    if !valid_id(&id) {
        return Err("id: must be 3-48 characters of a-z 0-9 . -".into());
    }
    if id.starts_with("axidps.") {
        return Err("axidps.* ids are reserved for bundled plugins".into());
    }
    let mut versions: Vec<(semver::Version, Version)> = Vec::new();
    let list = o.get("versions").and_then(Value::as_array).ok_or("versions: not a list")?;
    for item in list {
        let label = item.get("version").and_then(Value::as_str).unwrap_or("?");
        match version(item) {
            Err(e) => skipped.push(clean(&format!("{id} {label} skipped: {e}"))),
            Ok((sv, _)) if versions.iter().any(|(s, _)| *s == sv) => {
                skipped.push(clean(&format!("{id} {label} skipped: listed twice")))
            }
            Ok(pair) => versions.push(pair),
        }
    }
    if versions.is_empty() {
        return Err("no usable versions".into());
    }
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Listing {
        name: opt_str(o, "name")?.unwrap_or_else(|| id.clone()),
        author: opt_str(o, "author")?.unwrap_or_default(),
        description: opt_str(o, "description")?.unwrap_or_default(),
        icon: opt_str(o, "icon")?.unwrap_or_default(),
        repo: opt_str(o, "repo")?.unwrap_or_default(),
        id,
        versions: versions.into_iter().map(|(_, v)| v).collect(),
    })
}

fn version(v: &Value) -> Result<(semver::Version, Version), String> {
    let o = v.as_object().ok_or("not an object")?;
    let version = str_of(o, "version")?;
    let sv = semver::Version::parse(&version)
        .map_err(|_| format!("version: \"{version}\" is not a semantic version"))?;
    let url = str_of(o, "url")?;
    if !url.starts_with("https://") {
        return Err("url: must be https://".into());
    }
    let sha256 = str_of(o, "sha256")?;
    let tree = str_of(o, "tree")?;
    for (f, h) in [("sha256", &sha256), ("tree", &tree)] {
        if !is_hash(h) {
            return Err(format!("{f}: not 64 lowercase hex characters"));
        }
    }
    let api = o
        .get("api")
        .and_then(Value::as_u64)
        .and_then(|a| u32::try_from(a).ok())
        .ok_or("api: not a number")?;
    let ver = Version {
        version,
        url,
        sha256,
        tree,
        permissions: list_of(o, "permissions")?,
        network: list_of(o, "network")?,
        api,
    };
    Ok((sv, ver))
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn str_of(o: &Map<String, Value>, f: &str) -> Result<String, String> {
    o.get(f)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{f}: not a string"))
}

fn opt_str(o: &Map<String, Value>, f: &str) -> Result<Option<String>, String> {
    match o.get(f) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{f}: not a string")),
    }
}

/// An optional list of strings; missing means empty.
fn list_of(o: &Map<String, Value>, f: &str) -> Result<Vec<String>, String> {
    match o.get(f) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| format!("{f}: not a list of strings")))
            .collect(),
        Some(_) => Err(format!("{f}: not a list")),
    }
}

impl Listing {
    /// Pulled by `revoked.json` (the whole id, or this exact version and tree): never offered.
    fn pulled(&self, v: &Version, signed: &Signed) -> bool {
        signed.revoked.as_ref().is_some_and(|r| {
            r.ids.iter().any(|(id, _)| *id == self.id)
                || r.versions
                    .iter()
                    .any(|(id, ver, t, _)| *id == self.id && *ver == v.version && *t == v.tree)
        })
    }

    /// Has a verified approval for this exact version and tree that hasn't been revoked.
    pub fn approved(&self, v: &Version, signed: &Signed) -> bool {
        let lost = signed.revoked.as_ref().is_some_and(|r| {
            r.verifications.iter().any(|(id, ver, _)| *id == self.id && *ver == v.version)
        });
        !lost
            && !self.pulled(v, signed)
            && signed.approvals.iter().any(|a| {
                a.verified && a.id == self.id && a.version == v.version && a.tree == v.tree
            })
    }

    /// How far `v` is cleared to load with its deeper permissions: Verified by its own
    /// approval, or Approved when a standing approval for this plugin (not revoked) grants every
    /// deeper permission it asks for, and it asks for at least one. None otherwise.
    pub fn cleared(&self, v: &Version, signed: &Signed) -> Option<Trust> {
        if self.approved(v, signed) {
            return Some(Trust::Verified);
        }
        // A revoked verification takes the version's grants away, standing or not.
        let lost = signed.revoked.as_ref().is_some_and(|r| {
            r.verifications.iter().any(|(id, ver, _)| *id == self.id && *ver == v.version)
        });
        let grant = &signed.live_standing(&self.id)?.grant;
        let deeper: Vec<&String> =
            v.permissions.iter().filter(|p| level(p) == Some(Level::Deeper)).collect();
        let covered = !deeper.is_empty() && deeper.iter().all(|p| grant.contains(p));
        (!lost && !self.pulled(v, signed) && covered).then_some(Trust::Approved)
    }

    /// The version the store offers: the newest cleared one (approved, or covered by a
    /// standing approval) when any is, else the newest. Revoked versions are never offered.
    pub fn offer(&self, signed: &Signed) -> Option<&Version> {
        let live = || self.versions.iter().rev().filter(|v| !self.pulled(v, signed));
        live().find(|v| self.cleared(v, signed).is_some()).or_else(|| live().next())
    }

    /// A version newer than the offered (cleared) one is waiting for review.
    pub fn awaiting_review(&self, signed: &Signed) -> bool {
        let Some(offered) = self.offer(signed) else { return false };
        self.versions
            .iter()
            .rev()
            .filter(|v| !self.pulled(v, signed))
            .take_while(|v| !std::ptr::eq(*v, offered))
            .next()
            .is_some()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sign::{canonical, key_id};
    use crate::trust::{Approval, Revoked};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    pub(crate) fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    pub(crate) fn keys() -> Keys {
        let pk = key().verifying_key().to_bytes();
        Keys { approvers: vec![pk], index: vec![pk] }
    }

    pub(crate) fn sign_index(bytes: &[u8]) -> String {
        let mut msg = INDEX_PREFIX.as_bytes().to_vec();
        msg.extend_from_slice(bytes);
        STANDARD.encode(key().sign(&msg).to_bytes())
    }

    fn approval(id: &str, version: &str, tree: &str) -> Value {
        let mut o = json!({
            "id": id, "version": version, "tree": tree, "verified": true, "grant": [],
            "approver": key_id(&key().verifying_key().to_bytes()),
        });
        let msg = format!("axidps-approval-v1\n{}", canonical(&o).unwrap());
        o["sig"] = json!(STANDARD.encode(key().sign(msg.as_bytes()).to_bytes()));
        o
    }

    fn h(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn ver(v: &str, tree: char) -> Value {
        json!({
            "version": v, "url": format!("https://example.com/{v}.zip"), "sha256": h('a'),
            "tree": h(tree), "permissions": ["fight"], "network": [], "api": 1,
        })
    }

    fn sample() -> Vec<u8> {
        let v = json!({
            "generated": "2026-11-26T00:00:00Z",
            "plugins": [
                {"id": "someone.card", "name": "Card", "author": "someone",
                 "versions": [ver("1.0.0", 'b'), ver("1.2.0", 'c'), ver("1.10.0", 'd')]},
                {"id": "axidps.x", "name": "Fake", "versions": [ver("1.0.0", 'b')]},
                {"id": "Bad Id", "versions": [ver("1.0.0", 'b')]},
                {"id": "someone.half", "versions": [ver("1.0.0", 'b'), ver("one", 'b'),
                    {"version": "2.0.0", "url": "http://x/", "sha256": h('a'), "tree": h('b'), "api": 1}]},
            ],
            "approvals": [approval("someone.card", "1.2.0", &h('c'))],
        });
        serde_json::to_vec_pretty(&v).unwrap()
    }

    #[test]
    fn index_signature_required() {
        let bytes = sample();
        let sig = sign_index(&bytes);
        let idx = parse(&bytes, &sig, &keys()).unwrap();
        let ids: Vec<&str> = idx.plugins.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["someone.card", "someone.half"]);
        assert_eq!(idx.generated, "2026-11-26T00:00:00Z");
        assert!(idx.skipped.iter().any(|s| s.contains("axidps.x")), "{:?}", idx.skipped);
        assert_eq!(idx.plugins[1].versions.len(), 1, "bad versions skipped");
        // Semver order, not string order.
        let vs: Vec<&str> = idx.plugins[0].versions.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(vs, ["1.0.0", "1.2.0", "1.10.0"]);
        assert!(idx.raw.get("approvals").is_some());

        // Any one changed byte fails.
        for i in [0, bytes.len() / 2, bytes.len() - 1] {
            let mut t = bytes.clone();
            t[i] ^= 1;
            assert!(parse(&t, &sig, &keys()).is_err(), "byte {i}");
        }
        // The approver-only keys can't sign the index.
        let k = Keys { approvers: keys().approvers, index: vec![] };
        assert!(parse(&bytes, &sig, &k).is_err());
        assert!(parse(&bytes, "", &keys()).is_err());
    }

    fn version_of(v: &str, perms: &[&str]) -> Version {
        Version {
            version: v.into(),
            url: format!("https://example.com/{v}.zip"),
            sha256: "0".repeat(64),
            tree: h('a'),
            permissions: perms.iter().map(|p| p.to_string()).collect(),
            network: vec![],
            api: 1,
        }
    }

    fn textures_listing() -> Listing {
        Listing {
            id: "someone.tags".into(),
            name: "Tags".into(),
            author: "someone".into(),
            description: String::new(),
            icon: "plug".into(),
            repo: "someone/tags".into(),
            versions: vec![version_of("1.0.0", &["textures"]), version_of("1.1.0", &["textures"]),
                           version_of("1.2.0", &["textures", "world"])],
        }
    }

    fn standing_for(id: &str, grant: &[&str]) -> Signed {
        Signed {
            standing: vec![crate::trust::Standing {
                id: id.into(),
                grant: grant.iter().map(|g| g.to_string()).collect(),
                sig: "c2ln".into(),
            }],
            ..Signed::default()
        }
    }

    #[test]
    fn offer_takes_standing_covered_version() {
        let l = textures_listing();
        let s = standing_for("someone.tags", &["textures"]);
        assert_eq!(l.offer(&s).unwrap().version, "1.1.0");
        assert_eq!(l.cleared(&l.versions[1], &s), Some(Trust::Approved));
        assert_eq!(l.cleared(&l.versions[1], &standing_for("other.id", &["textures"])), None);
    }

    #[test]
    fn standing_doesnt_clear_extra_permission() {
        let l = textures_listing();
        let s = standing_for("someone.tags", &["textures"]);
        assert_eq!(l.cleared(&l.versions[2], &s), None, "world isn't in the grant");
        assert!(l.awaiting_review(&s), "1.2.0 waits for review");
        let mut gone = s.clone();
        gone.revoked = Some(Revoked {
            seq: 1,
            standing: vec![("someone.tags".into(), "c2ln".into(), "why".into())],
            ..Revoked::default()
        });
        assert_eq!(l.cleared(&l.versions[1], &gone), None, "a revoked standing approval clears nothing");
    }

    #[test]
    fn revoking_another_standing_object_doesnt_matter() {
        let l = textures_listing();
        let mut s = standing_for("someone.tags", &["textures"]);
        s.revoked = Some(Revoked {
            seq: 1,
            standing: vec![("someone.tags".into(), "b2xk".into(), "replaced".into())],
            ..Revoked::default()
        });
        assert_eq!(l.cleared(&l.versions[1], &s), Some(Trust::Approved));
    }

    #[test]
    fn verification_revoked_isnt_cleared_by_standing() {
        let l = textures_listing();
        let mut s = standing_for("someone.tags", &["textures"]);
        s.revoked = Some(Revoked {
            seq: 1,
            verifications: vec![("someone.tags".into(), "1.1.0".into(), "bad".into())],
            ..Revoked::default()
        });
        assert_eq!(l.cleared(&l.versions[1], &s), None);
        assert_eq!(l.offer(&s).unwrap().version, "1.0.0");
    }

    #[test]
    fn cover_lists_the_versions_standing_clears() {
        let l = textures_listing();
        let mut s = standing_for("someone.tags", &["textures"]);
        s.cover(std::slice::from_ref(&l));
        let want: HashSet<(String, String, String)> = ["1.0.0", "1.1.0"]
            .iter()
            .map(|v| ("someone.tags".to_string(), v.to_string(), h('a')))
            .collect();
        assert_eq!(s.covered, want, "1.2.0 asks for world");
    }

    #[test]
    fn awaiting_review_false_when_standing_covers() {
        let mut l = textures_listing();
        l.versions.pop();
        let s = standing_for("someone.tags", &["textures", "world"]);
        assert_eq!(l.offer(&s).unwrap().version, "1.1.0");
        assert!(!l.awaiting_review(&s));
        // A version asking for no deeper permission isn't "approved" by a standing approval.
        l.versions.push(version_of("1.3.0", &["fight"]));
        assert_eq!(l.cleared(&l.versions[2], &s), None);
    }

    #[test]
    fn offer_prefers_last_approved() {
        let bytes = sample();
        let idx = parse(&bytes, &sign_index(&bytes), &keys()).unwrap();
        let card = &idx.plugins[0];
        let (signed, log) = Signed::from_index(&idx.raw, &keys(), 0);
        assert!(log.is_empty(), "{log:?}");

        // 1.2.0 is approved; the newer 1.10.0 waits for review.
        assert_eq!(card.offer(&signed).unwrap().version, "1.2.0");
        assert!(card.awaiting_review(&signed));

        // Nothing approved: newest, nothing awaiting.
        let none = Signed::default();
        assert_eq!(card.offer(&none).unwrap().version, "1.10.0");
        assert!(!card.awaiting_review(&none));

        // Newest approved, nothing newer: not awaiting.
        let mut s = signed.clone();
        s.approvals.push(Approval {
            id: "someone.card".into(),
            version: "1.10.0".into(),
            tree: h('d'),
            verified: true,
            grant: vec![],
        });
        assert_eq!(card.offer(&s).unwrap().version, "1.10.0");
        assert!(!card.awaiting_review(&s));

        // An approval for another tree, or unverified, doesn't count.
        let mut s = Signed::default();
        s.approvals.push(Approval {
            id: "someone.card".into(),
            version: "1.2.0".into(),
            tree: h('e'),
            verified: true,
            grant: vec![],
        });
        s.approvals.push(Approval {
            id: "someone.card".into(),
            version: "1.0.0".into(),
            tree: h('b'),
            verified: false,
            grant: vec![],
        });
        assert_eq!(card.offer(&s).unwrap().version, "1.10.0");

        // A revoked verification falls back to the previous approval... none here, so newest.
        let mut s = signed.clone();
        s.revoked = Some(Revoked {
            seq: 1,
            verifications: vec![("someone.card".into(), "1.2.0".into(), "why".into())],
            ..Revoked::default()
        });
        assert_eq!(card.offer(&s).unwrap().version, "1.10.0");

        // A pulled version is never offered; a pulled id offers nothing.
        let mut s = Signed {
            revoked: Some(Revoked {
                seq: 1,
                versions: vec![("someone.card".into(), "1.10.0".into(), h('d'), "bad".into())],
                ..Revoked::default()
            }),
            ..Signed::default()
        };
        assert_eq!(card.offer(&s).unwrap().version, "1.2.0");
        s.revoked.as_mut().unwrap().ids.push(("someone.card".into(), "bad".into()));
        assert!(card.offer(&s).is_none());
        assert!(!card.awaiting_review(&s));
    }
}
