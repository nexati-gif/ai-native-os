//! Federation: cross-device sync

pub mod federator;
pub mod automerge_sync;
pub mod git_sync;
pub mod tailscale_client;

pub use federator::{Federator, FederationConfig};
pub use automerge_sync::AutomergeSync;
pub use git_sync::GitSync;
pub use tailscale_client::TailscaleClient;