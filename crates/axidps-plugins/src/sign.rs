//! Canonical JSON and Ed25519 checks for store approvals, passes, revocations and the index.


use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Canonical JSON: keys sorted by bytes, no whitespace, strings printable ASCII only and
/// numbers non-negative integers only. Anything else is refused rather than escaped.
pub fn canonical(v: &Value) -> Result<String, String> {
    let mut out = String::new();
    write(v, &mut out)?;
    Ok(out)
}

/// Key id: lowercase hex of the first 8 bytes of SHA-256 of the public key.
pub fn key_id(pk: &[u8; 32]) -> String {
    Sha256::digest(pk)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks a signed approval, pass or revocation: `sig` over `axidps-<kind>-v1\n` + the
/// canonical object without `sig`, by the trusted key whose id is `approver`.
pub fn verify_object(v: &Value, kind: &str, keys: &[[u8; 32]]) -> Result<(), String> {
    let obj = v.as_object().ok_or("signed object: not an object")?;
    let sig = obj.get("sig").and_then(Value::as_str).ok_or("signed object: no sig")?;
    let approver = obj
        .get("approver")
        .and_then(Value::as_str)
        .ok_or("signed object: no approver")?;
    let mut body = obj.clone();
    body.remove("sig");
    let data = canonical(&Value::Object(body))?;
    let trusted: Vec<[u8; 32]> = keys.iter().filter(|k| key_id(k) == approver).copied().collect();
    if trusted.is_empty() {
        return Err(format!("signed object: approver {approver} isn't a trusted key"));
    }
    if verify_bytes(&format!("axidps-{kind}-v1\n"), data.as_bytes(), sig, &trusted) {
        Ok(())
    } else {
        Err(format!("signed object: bad {kind} signature"))
    }
}

/// True if `sig_b64` (standard padded base64) is a valid signature over `prefix` + `data`
/// by any of `keys`.
pub fn verify_bytes(prefix: &str, data: &[u8], sig_b64: &str, keys: &[[u8; 32]]) -> bool {
    let Ok(raw) = STANDARD.decode(sig_b64) else { return false };
    let Ok(raw) = <[u8; 64]>::try_from(raw.as_slice()) else { return false };
    let sig = Signature::from_bytes(&raw);
    let mut msg = Vec::with_capacity(prefix.len() + data.len());
    msg.extend_from_slice(prefix.as_bytes());
    msg.extend_from_slice(data);
    keys.iter().any(|k| {
        VerifyingKey::from_bytes(k).is_ok_and(|vk| vk.verify_strict(&msg, &sig).is_ok())
    })
}

fn write(v: &Value, out: &mut String) -> Result<(), String> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match n.as_u64() {
            Some(n) => out.push_str(&n.to_string()),
            None => return Err(format!("canonical JSON: {n} isn't a non-negative integer")),
        },
        Value::String(s) => write_str(s, out)?,
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out)?;
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out)?;
                out.push(':');
                write(&m[k], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_str(s: &str, out: &mut String) -> Result<(), String> {
    if let Some(c) = s.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(format!("canonical JSON: {c:?} isn't printable ASCII"));
    }
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../tests/vectors.json")).unwrap()
    }

    fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn pub_of(case: &Value) -> [u8; 32] {
        STANDARD.decode(case["pub"].as_str().unwrap()).unwrap().try_into().unwrap()
    }

    #[test]
    fn canonical_vectors() {
        let v = vectors();
        let cases = v["canonical"].as_array().unwrap();
        for case in cases {
            let got = canonical(&case["in"]).ok();
            assert_eq!(got.as_deref(), case["out"].as_str(), "{}", case["in"]);
        }
        let refused = cases.iter().filter(|c| c["out"].is_null()).count();
        assert!(refused >= 4);
    }

    #[test]
    fn canonical_sorts_keys_and_refuses_the_rest() {
        assert_eq!(canonical(&json!({"b": 1, "a": "x"})).unwrap(), r#"{"a":"x","b":1}"#);
        for bad in [json!("é"), json!(1.5), json!(-1), json!("\n"), json!({"\n": 1}), json!([1.0])] {
            assert!(canonical(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn key_id_is_first_8_bytes_of_sha256_hex() {
        let pk = test_key().verifying_key().to_bytes();
        let id = key_id(&pk);
        assert_eq!(id.len(), 16);
        assert!(id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let v = vectors();
        assert_eq!(v["signed"][0]["object"]["approver"], id);
    }

    #[test]
    fn signed_vectors_verify() {
        let v = vectors();
        let cases = v["signed"].as_array().unwrap();
        let kinds: Vec<&str> = cases.iter().map(|c| c["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["approval", "pass", "revoked"]);
        for case in cases {
            let pk = pub_of(case);
            verify_object(&case["object"], case["kind"].as_str().unwrap(), &[pk]).unwrap();
        }
    }

    #[test]
    fn openssl_signatures_match_rust() {
        // The vectors were signed by `openssl pkeyutl -sign -rawin` from a PEM of seed [7; 32];
        // Ed25519 is deterministic, so Rust signing the same bytes gives the same signature.
        let key = test_key();
        let v = vectors();
        for case in v["signed"].as_array().unwrap() {
            assert_eq!(pub_of(case), key.verifying_key().to_bytes());
            let kind = case["kind"].as_str().unwrap();
            let mut body = case["object"].clone();
            let sig = body.as_object_mut().unwrap().remove("sig").unwrap();
            let msg = format!("axidps-{kind}-v1\n{}", canonical(&body).unwrap());
            let ours = STANDARD.encode(key.sign(msg.as_bytes()).to_bytes());
            assert_eq!(ours, sig.as_str().unwrap(), "{kind}");
        }
    }

    #[test]
    fn any_flipped_byte_fails() {
        let v = vectors();
        let case = &v["signed"][0];
        let pk = pub_of(case);
        let obj = &case["object"];
        let kind = case["kind"].as_str().unwrap();

        // Every byte of every string field (except the approver id, checked below).
        for (field, val) in obj.as_object().unwrap() {
            if field == "approver" {
                continue;
            }
            let Some(s) = val.as_str() else { continue };
            for i in 0..s.len() {
                let mut bytes = s.as_bytes().to_vec();
                bytes[i] = if bytes[i] == b'A' { b'B' } else { b'A' };
                let mut t = obj.clone();
                t[field.as_str()] = Value::String(String::from_utf8(bytes).unwrap());
                assert!(verify_object(&t, kind, &[pk]).is_err(), "{field}[{i}]");
            }
        }
        // Every byte of the raw signature.
        let sig = STANDARD.decode(obj["sig"].as_str().unwrap()).unwrap();
        for i in 0..sig.len() {
            let mut s = sig.clone();
            s[i] ^= 1;
            let mut t = obj.clone();
            t["sig"] = Value::String(STANDARD.encode(&s));
            assert!(verify_object(&t, kind, &[pk]).is_err(), "sig[{i}]");
        }
        // Numbers and added fields count too; so does the kind.
        let mut t = obj.clone();
        t["issued"] = json!(obj["issued"].as_u64().unwrap() + 1);
        assert!(verify_object(&t, kind, &[pk]).is_err());
        let mut t = obj.clone();
        t["extra"] = json!(1);
        assert!(verify_object(&t, kind, &[pk]).is_err());
        assert!(verify_object(obj, "pass", &[pk]).is_err());
        verify_object(obj, kind, &[pk]).unwrap();
    }

    #[test]
    fn wrong_approver_or_no_keys_fails() {
        let v = vectors();
        let case = &v["signed"][1];
        let pk = pub_of(case);
        let kind = case["kind"].as_str().unwrap();
        let mut t = case["object"].clone();
        t["approver"] = json!("0000000000000000");
        assert!(verify_object(&t, kind, &[pk]).is_err());
        assert!(verify_object(&case["object"], kind, &[]).is_err());
        // A different trusted key with a different id: not found.
        let other = SigningKey::from_bytes(&[8u8; 32]).verifying_key().to_bytes();
        assert!(verify_object(&case["object"], kind, &[other]).is_err());
        verify_object(&case["object"], kind, &[other, pk]).unwrap();
    }

    #[test]
    fn malformed_objects_fail() {
        let v = vectors();
        let case = &v["signed"][2];
        let pk = pub_of(case);
        let obj = &case["object"];
        for field in ["sig", "approver"] {
            let mut t = obj.clone();
            t.as_object_mut().unwrap().remove(field);
            assert!(verify_object(&t, "revoked", &[pk]).is_err(), "{field}");
        }
        let mut t = obj.clone();
        t["sig"] = json!("not base64!");
        assert!(verify_object(&t, "revoked", &[pk]).is_err());
        let mut t = obj.clone();
        t["sig"] = json!(STANDARD.encode([0u8; 63]));
        assert!(verify_object(&t, "revoked", &[pk]).is_err());
        assert!(verify_object(&json!([1]), "revoked", &[pk]).is_err());
        // An object that can't be made canonical is refused, not signed over loosely.
        let mut t = obj.clone();
        t["note"] = json!("é");
        assert!(verify_object(&t, "revoked", &[pk]).is_err());
    }

    #[test]
    fn verify_bytes_checks_prefix_data_and_key() {
        let key = test_key();
        let pk = key.verifying_key().to_bytes();
        let data = br#"{"plugins":[]}"#;
        let mut msg = b"axidps-index-v1\n".to_vec();
        msg.extend_from_slice(data);
        let sig = STANDARD.encode(key.sign(&msg).to_bytes());
        assert!(verify_bytes("axidps-index-v1\n", data, &sig, &[pk]));
        let other = SigningKey::from_bytes(&[8u8; 32]).verifying_key().to_bytes();
        assert!(verify_bytes("axidps-index-v1\n", data, &sig, &[other, pk]));
        assert!(!verify_bytes("axidps-index-v1\n", data, &sig, &[other]));
        assert!(!verify_bytes("axidps-index-v1\n", data, &sig, &[]));
        assert!(!verify_bytes("axidps-approval-v1\n", data, &sig, &[pk]));
        assert!(!verify_bytes("axidps-index-v1\n", br#"{"plugins":[1]}"#, &sig, &[pk]));
        assert!(!verify_bytes("axidps-index-v1\n", data, "garbage", &[pk]));
        assert!(!verify_bytes("axidps-index-v1\n", data, &sig[..sig.len() - 1], &[pk]));
    }
}
