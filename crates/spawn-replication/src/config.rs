//! Driver configuration.

use crate::snapshot::SEND_BUDGET_BYTES;

/// Tunables for the server and client drivers (decision 8 defaults).
#[derive(Debug, Clone, Copy)]
pub struct ReplicationConfig {
    /// Per-tick replication send budget in bytes (bounds the snapshot updates section).
    pub send_budget: usize,
    /// Default per-client interest `view_radius` (the game may override per client).
    pub default_view_radius: f32,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            send_budget: SEND_BUDGET_BYTES,
            default_view_radius: 32.0,
        }
    }
}
