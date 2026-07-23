//! Watches `~/.ssh/config` for changes made outside the app (a text editor, `scp`, another
//! machine syncing dotfiles) so the frontend can reload its host list.
//!
//! No `glib`/tokio dependency here: this crate stays toolkit-agnostic. The receiver is a plain
//! `std::sync::mpsc::Receiver`; bridging it into a GUI event loop is the frontend's job.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("falha ao observar {path}: {source}")]
    Notify { path: PathBuf, source: notify::Error },
}

/// Watches a config file's parent directory (not the file itself) because editors typically
/// replace it atomically — write a temp file, then rename over the original — which would
/// otherwise orphan a watch on the old inode.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<()>,
}

impl ConfigWatcher {
    pub fn watch(path: impl AsRef<Path>) -> Result<Self, WatchError> {
        let path = path.as_ref().to_path_buf();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let (tx, rx) = mpsc::channel();

        let target = path.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                return;
            }
            if event.paths.iter().any(|p| p == &target) {
                let _ = tx.send(());
            }
        })
        .map_err(|e| WatchError::Notify { path: dir.clone(), source: e })?;

        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .map_err(|e| WatchError::Notify { path: dir.clone(), source: e })?;

        Ok(Self { _watcher: watcher, receiver: rx })
    }

    /// A change was observed since the last call. Non-blocking.
    pub fn poll(&self) -> bool {
        // Drain every pending event so a burst of writes (common with atomic replace) only ever
        // reports "changed" once per poll instead of queuing up repeats.
        let mut changed = false;
        while self.receiver.try_recv().is_ok() {
            changed = true;
        }
        changed
    }

    /// The receiving half, for a caller that wants to bridge events into its own event loop
    /// (e.g. a background thread forwarding into a GLib main-context channel) instead of
    /// polling.
    pub fn receiver(&self) -> &Receiver<()> {
        &self.receiver
    }
}

/// A short, fixed debounce so a burst of filesystem events from one atomic save collapses into a
/// single reload instead of several in quick succession.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;

    #[test]
    fn detects_atomic_replace_of_the_watched_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config");
        fs::write(&path, "Host a\n").unwrap();

        let watcher = ConfigWatcher::watch(&path).expect("watch");
        thread::sleep(DEBOUNCE);
        assert!(!watcher.poll(), "no changes yet");

        // Atomic replace: write a temp file, then rename over the original, exactly like an
        // editor's "safe save".
        let tmp = dir.path().join("config.tmp");
        fs::write(&tmp, "Host b\n").unwrap();
        fs::rename(&tmp, &path).unwrap();

        thread::sleep(DEBOUNCE);
        assert!(watcher.poll(), "rename over the watched path should be observed");
    }
}
