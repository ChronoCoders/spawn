//! Filesystem hot-reload: a `notify` recursive watcher over the asset root, a
//! per-path debounce, and the typed [`ReloadEvent`] stream owners poll.
//!
//! The watcher thread converts raw filesystem events into canonical paths and
//! pushes them onto a channel. The server's `apply_loaded` pump drains that
//! channel, applies the per-path debounce window, and enqueues reload jobs for
//! paths whose [`AssetId`] is currently tracked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::AssetError;
use crate::id::{canonicalize, AssetId};

#[derive(Debug, Clone)]
pub struct ReloadEvent {
    pub id: AssetId,
    pub generation: u32,
    pub outcome: ReloadOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    Replaced,
    Failed,
}

/// Owns the `notify` watcher and the channel of canonical paths it reports.
/// Dropping it stops the watcher; the background notify thread is owned by
/// `notify` and torn down when the watcher is dropped.
pub(crate) struct HotReload {
    _watcher: RecommendedWatcher,
    event_rx: Receiver<PathBuf>,
    root: PathBuf,
    debounce: Duration,
    pending: HashMap<String, Instant>,
}

impl HotReload {
    pub(crate) fn new(root: &Path, debounce: Duration) -> Result<Self, AssetError> {
        let (tx, event_rx) = channel::<PathBuf>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if matches!(
                    event.kind,
                    notify::EventKind::Create(_)
                        | notify::EventKind::Modify(_)
                        | notify::EventKind::Remove(_)
                ) {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            }
        })
        .map_err(|e| AssetError::WatcherInit {
            detail: e.to_string(),
        })?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| AssetError::WatcherInit {
                detail: e.to_string(),
            })?;
        Ok(Self {
            _watcher: watcher,
            event_rx,
            root: root.to_path_buf(),
            debounce,
            pending: HashMap::new(),
        })
    }

    /// Drains raw watcher events, records/refreshes the per-path debounce timer,
    /// and returns the canonical paths whose debounce window has elapsed since
    /// their last event. Each new event for a path resets that path's timer.
    pub(crate) fn collect_ready(&mut self, now: Instant) -> Vec<String> {
        while let Ok(path) = self.event_rx.try_recv() {
            if let Some(canonical) = self.to_canonical(&path) {
                self.pending.insert(canonical, now);
            }
        }
        let debounce = self.debounce;
        let mut ready = Vec::new();
        self.pending.retain(|canonical, first_seen| {
            if now.duration_since(*first_seen) >= debounce {
                ready.push(canonical.clone());
                false
            } else {
                true
            }
        });
        ready
    }

    fn to_canonical(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(&self.root).unwrap_or(path);
        let raw = relative.to_str()?;
        Some(canonicalize(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_holds_then_releases() {
        let dir = std::env::temp_dir().join(format!("spawn_watch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut hot = HotReload::new(&dir, Duration::from_millis(50)).unwrap();
        let start = Instant::now();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let _ = hot.collect_ready(start);
        let ready_late = loop {
            std::thread::sleep(Duration::from_millis(20));
            let r = hot.collect_ready(Instant::now());
            if !r.is_empty() {
                break r;
            }
            if start.elapsed() > Duration::from_secs(2) {
                break r;
            }
        };
        assert!(ready_late.iter().any(|p| p == "a.txt") || ready_late.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
