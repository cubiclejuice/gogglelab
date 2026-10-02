use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct LoadedModel {
    pub generation: u64,
    pub path: String,
    pub parsed: Arc<stl_core::ParsedModel>,
}

/// Single-slot model cache (spec §2 — one-file-at-a-time is the whole scope,
/// so a generation-keyed map would only ever be an unbounded leak) plus the
/// generation ledger that makes staleness a backend guarantee (R-02): a
/// response commits only if its generation is still the latest one requested
/// at the moment it finishes, so a slow load can never overwrite a newer one.
#[derive(Default)]
pub struct AppState {
    pub latest_requested: AtomicU64,
    /// Serializes request registration with the final active-model commit.
    /// A load may do I/O and measurement outside this lock, but it must not
    /// check freshness and publish in two independently ordered operations.
    pub request_commit: Mutex<()>,
    pub current: Mutex<Option<LoadedModel>>,
}

impl AppState {
    pub fn register_generation(&self, generation: u64) {
        let _guard = self.request_commit.lock().unwrap();
        self.latest_requested
            .fetch_max(generation, Ordering::SeqCst);
    }

    pub fn is_latest_locked(&self, generation: u64) -> bool {
        generation == self.latest_requested.load(Ordering::SeqCst)
    }
}
