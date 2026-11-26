//! Heal/barrier relay: the wire format shared with the relay server, the room id, and the
//! client thread that owns the socket.

pub mod client;
pub mod link;
pub mod wire;

/// The heal relay's base URL.
pub const DEFAULT_RELAY_URL: &str = "wss://axidps-relay.project96.workers.dev";
