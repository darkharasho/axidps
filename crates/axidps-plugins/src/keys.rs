//! The public keys this build trusts for the plugin store.


/// Trusted public keys. Approver keys sign approvals, passes and `revoked.json`; index keys
/// sign `index.json` only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keys {
    pub approvers: Vec<[u8; 32]>,
    pub index: Vec<[u8; 32]>,
}

/// The approver public key. Its private half lives in 1Password only.
const APPROVER_KEYS: &[[u8; 32]] = &[[
    0xf2, 0xcf, 0x27, 0x0e, 0x05, 0xc3, 0xa5, 0xeb, 0x13, 0x99, 0x81, 0x0b, 0x38, 0x04, 0x32, 0xa3,
    0x93, 0xf7, 0xb7, 0xe7, 0x5d, 0x83, 0xe5, 0x98, 0x32, 0x2c, 0x88, 0x09, 0xe8, 0x57, 0xa2, 0xf9,
]];
/// The index public key. Its private half is the registry repo's `INDEX_KEY` Actions secret.
const INDEX_KEYS: &[[u8; 32]] = &[[
    0xc0, 0x68, 0x4f, 0x9c, 0xd2, 0xd8, 0xfd, 0x23, 0xa7, 0x71, 0x78, 0x91, 0xf0, 0x4d, 0x0c, 0xac,
    0xef, 0xc7, 0xa3, 0xc8, 0xa0, 0x08, 0x7a, 0xe1, 0x38, 0x26, 0x2e, 0x50, 0x0a, 0x71, 0xb9, 0x28,
]];

impl Keys {
    /// The keys compiled into this build.
    pub fn release() -> Keys {
        Keys { approvers: APPROVER_KEYS.to_vec(), index: INDEX_KEYS.to_vec() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use base64::Engine;

    fn parse(text: &str) -> Vec<[u8; 32]> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let raw = base64::engine::general_purpose::STANDARD.decode(l).unwrap();
                <[u8; 32]>::try_from(raw.as_slice()).unwrap()
            })
            .collect()
    }

    #[test]
    fn release_keys_match_registry_files() {
        let approvers = parse(include_str!("../tests/registry-keys/approvers.txt"));
        let index = parse(include_str!("../tests/registry-keys/index.txt"));
        let k = Keys::release();
        assert_eq!(approvers.len(), 1);
        assert_eq!(index.len(), 1);
        assert_eq!(k.approvers, approvers);
        assert_eq!(k.index, index);
        assert_ne!(k.approvers[0], k.index[0]);
        assert_ne!(crate::sign::key_id(&k.approvers[0]), crate::sign::key_id(&k.index[0]));
    }
}
