//! The public keys this build trusts for the plugin store.


/// Trusted public keys. Approver keys sign approvals, passes and `revoked.json`; index keys
/// sign `index.json` only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keys {
    pub approvers: Vec<[u8; 32]>,
    pub index: Vec<[u8; 32]>,
}

const APPROVER_KEYS: &[[u8; 32]] = &[];
const INDEX_KEYS: &[[u8; 32]] = &[];

impl Keys {
    /// The keys compiled into this build.
    pub fn release() -> Keys {
        Keys { approvers: APPROVER_KEYS.to_vec(), index: INDEX_KEYS.to_vec() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_keys_are_empty_until_published() {
        let k = Keys::release();
        assert!(k.approvers.is_empty());
        assert!(k.index.is_empty());
    }
}
