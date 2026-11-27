//! Plugin plumbing that needs no Servo: manifests, folder scan, path resolution, CSP and a
//! change watcher. The overlay host and `axidps plugin check/preview` share it.

pub mod index;
pub mod install;
pub mod keys;
pub mod logline;
pub mod manifest;
pub mod scan;
pub mod serve;
pub mod sign;
pub mod tree;
pub mod trust;
pub mod watch;

/// The manifest's file name inside a plugin folder.
pub const MANIFEST_FILE: &str = "axidps-plugin.json";
/// The only `api` value this build loads.
pub const SUPPORTED_API: u32 = 1;
