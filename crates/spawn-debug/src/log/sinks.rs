//! Built-in log sinks: `StderrSink`, `FileSink` (size rotation), `RingBufferSink`.
//!
//! All sinks are `Send + Sync`; concurrent writes are serialized by an internal
//! `Mutex`. Write failures are counted in the global dropped-record statistic
//! (see [`Logger::dropped_records`](super::Logger::dropped_records)), never
//! propagated through `write`.

use super::{lock_recover, report_dropped, LogLevel, LogRecord, LogSink, ThreadTag};
use crate::error::{DebugError, DebugResult};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Record a dropped write into both the per-sink counter and the process-global
/// `Logger` counter. The per-sink counter is always accurate even when the sink
/// is used without an initialized `Logger`.
fn record_dropped(local: &AtomicU64) {
    local.fetch_add(1, Ordering::Relaxed);
    report_dropped();
}

fn format_line(record: &LogRecord<'_>) -> String {
    format!(
        "[{}] {} {}: {}\n",
        record.timestamp.as_millis(),
        record.level.as_str(),
        record.target,
        record.message
    )
}

/// Writes formatted lines to `io::stderr()` under an internal `Mutex`.
pub struct StderrSink {
    out: Mutex<std::io::Stderr>,
}

impl StderrSink {
    // Spec §1.6 sanctions only `new()`; a public `Default` impl is out of spec.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            out: Mutex::new(std::io::stderr()),
        }
    }
}

impl LogSink for StderrSink {
    fn write(&self, record: &LogRecord<'_>) {
        let line = format_line(record);
        let mut out = lock_recover(&self.out);
        if out.write_all(line.as_bytes()).is_err() {
            report_dropped();
        }
    }

    fn flush(&self) {
        let mut out = lock_recover(&self.out);
        let _ = out.flush();
    }
}

/// Configuration for a [`FileSink`].
pub struct FileSinkConfig {
    pub path: PathBuf,
    pub max_bytes: u64,
    pub max_files: u32,
}

struct FileState {
    file: File,
    size: u64,
}

/// Appends lines to a file with simple size-based rotation. Rotation and writes
/// are serialized by an internal `Mutex`. I/O errors are counted, never
/// propagated through `write`.
pub struct FileSink {
    state: Mutex<FileState>,
    path: PathBuf,
    max_bytes: u64,
    max_files: u32,
    dropped: AtomicU64,
}

impl FileSink {
    /// Create/open `config.path` for appending.
    ///
    /// Errors map to `DebugError::Io`. `max_bytes == 0` is rejected with
    /// `DebugError::InvalidConfig`. `max_files == 0` means never rotate.
    pub fn open(config: FileSinkConfig) -> DebugResult<Self> {
        if config.max_bytes == 0 {
            return Err(DebugError::InvalidConfig {
                context: "FileSink max_bytes must be non-zero",
            });
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&config.path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            state: Mutex::new(FileState { file, size }),
            path: config.path,
            max_bytes: config.max_bytes,
            max_files: config.max_files,
            dropped: AtomicU64::new(0),
        })
    }

    fn rotated_path(&self, index: u32) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }

    /// Rotate `path` -> `path.1` -> ... up to `max_files`, dropping the oldest.
    /// Best-effort: I/O errors here are folded into the dropped counter by the
    /// caller path.
    fn rotate(&self, state: &mut FileState) -> std::io::Result<()> {
        let _ = state.file.flush();
        // Drop the oldest, then shift each kept file up one index.
        let last = self.max_files;
        let oldest = self.rotated_path(last - 1);
        if oldest.exists() {
            std::fs::remove_file(&oldest)?;
        }
        let mut i = last - 1;
        while i > 1 {
            let from = self.rotated_path(i - 1);
            let to = self.rotated_path(i);
            if from.exists() {
                std::fs::rename(&from, &to)?;
            }
            i -= 1;
        }
        let to = self.rotated_path(1);
        std::fs::rename(&self.path, &to)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        state.file = file;
        state.size = 0;
        Ok(())
    }

    fn write_inner(&self, line: &[u8]) -> std::io::Result<()> {
        let mut state = lock_recover(&self.state);
        if self.max_files > 0
            && self.max_bytes > 0
            && state.size.saturating_add(line.len() as u64) > self.max_bytes
            && state.size > 0
        {
            self.rotate(&mut state)?;
        }
        state.file.write_all(line)?;
        state.size = state.size.saturating_add(line.len() as u64);
        Ok(())
    }
}

impl LogSink for FileSink {
    fn write(&self, record: &LogRecord<'_>) {
        let line = format_line(record);
        if self.write_inner(line.as_bytes()).is_err() {
            record_dropped(&self.dropped);
        }
    }

    fn flush(&self) {
        let mut state = lock_recover(&self.state);
        let _ = state.file.flush();
    }
}

/// An owned, materialized log record. The `message: String` is the only logging
/// allocation, confined to this opt-in retaining sink.
pub struct OwnedRecord {
    pub level: LogLevel,
    pub target: &'static str,
    pub message: String,
    pub timestamp: core::time::Duration,
    pub thread: ThreadTag,
}

impl OwnedRecord {
    fn from_record(record: &LogRecord<'_>) -> Self {
        Self {
            level: record.level,
            target: record.target,
            message: format!("{}", record.message),
            timestamp: record.timestamp,
            thread: record.thread,
        }
    }
}

struct Ring {
    buf: Vec<OwnedRecord>,
    head: usize,
    len: usize,
}

/// Retains the last `capacity` records for the future overlay console.
/// When full, the oldest record is overwritten (wraparound).
pub struct RingBufferSink {
    ring: Mutex<Ring>,
    capacity: usize,
}

impl RingBufferSink {
    /// Fixed ring of `capacity` (clamped to at least 1), pre-allocated.
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            ring: Mutex::new(Ring {
                buf: Vec::with_capacity(capacity),
                head: 0,
                len: 0,
            }),
            capacity,
        }
    }

    /// Oldest-to-newest copy of the current contents.
    pub fn snapshot(&self) -> Vec<OwnedRecord> {
        let ring = lock_recover(&self.ring);
        self.collect(&ring, ring.len)
    }

    /// The last `n` records (clamped to `len`), newest-last.
    pub fn tail(&self, n: usize) -> Vec<OwnedRecord> {
        let ring = lock_recover(&self.ring);
        let take = n.min(ring.len);
        self.collect(&ring, take)
    }

    fn collect(&self, ring: &Ring, take: usize) -> Vec<OwnedRecord> {
        let mut out = Vec::with_capacity(take);
        // Index of the oldest of the `take` newest records.
        let start = (ring.head + ring.len - take) % self.capacity.max(1);
        for i in 0..take {
            let idx = (start + i) % self.capacity.max(1);
            let r = &ring.buf[idx];
            out.push(OwnedRecord {
                level: r.level,
                target: r.target,
                message: r.message.clone(),
                timestamp: r.timestamp,
                thread: r.thread,
            });
        }
        out
    }

    pub fn len(&self) -> usize {
        lock_recover(&self.ring).len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn clear(&self) {
        let mut ring = lock_recover(&self.ring);
        ring.buf.clear();
        ring.head = 0;
        ring.len = 0;
    }
}

impl LogSink for RingBufferSink {
    fn write(&self, record: &LogRecord<'_>) {
        let owned = OwnedRecord::from_record(record);
        let mut ring = lock_recover(&self.ring);
        if ring.len < self.capacity {
            ring.buf.push(owned);
            ring.len += 1;
        } else {
            let head = ring.head;
            ring.buf[head] = owned;
            ring.head = (head + 1) % self.capacity;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;

    fn rec<'a>(msg: core::fmt::Arguments<'a>) -> LogRecord<'a> {
        LogRecord {
            level: LogLevel::Info,
            target: "test",
            message: msg,
            timestamp: Duration::from_millis(1),
            thread: ThreadTag(0),
        }
    }

    #[test]
    fn ring_wraparound_snapshot_and_tail() {
        let s = RingBufferSink::with_capacity(3);
        for i in 0..5u32 {
            s.write(&rec(format_args!("m{i}")));
        }
        let snap = s.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].message, "m2");
        assert_eq!(snap[1].message, "m3");
        assert_eq!(snap[2].message, "m4");

        let tail = s.tail(2);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].message, "m3");
        assert_eq!(tail[1].message, "m4");

        assert_eq!(s.len(), 3);
        assert_eq!(s.capacity(), 3);
        assert!(!s.is_empty());
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn ring_tail_clamps() {
        let s = RingBufferSink::with_capacity(4);
        s.write(&rec(format_args!("a")));
        s.write(&rec(format_args!("b")));
        let tail = s.tail(10);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].message, "a");
        assert_eq!(tail[1].message, "b");
    }

    #[test]
    fn ring_capacity_minimum_one() {
        let s = RingBufferSink::with_capacity(0);
        assert_eq!(s.capacity(), 1);
        s.write(&rec(format_args!("x")));
        s.write(&rec(format_args!("y")));
        let snap = s.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].message, "y");
    }

    #[test]
    fn file_sink_rejects_zero_max_bytes() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("spawn_dbg_zero_{}.log", std::process::id()));
        let err = FileSink::open(FileSinkConfig {
            path,
            max_bytes: 0,
            max_files: 3,
        });
        assert!(matches!(err, Err(DebugError::InvalidConfig { .. })));
    }

    #[test]
    fn file_sink_rotation() {
        let dir = std::env::temp_dir();
        let base = dir.join(format!("spawn_dbg_rot_{}.log", std::process::id()));
        // Clean any leftovers.
        for i in 0..5 {
            let p = if i == 0 {
                base.clone()
            } else {
                let mut n = base.clone().into_os_string();
                n.push(format!(".{i}"));
                PathBuf::from(n)
            };
            let _ = std::fs::remove_file(p);
        }
        let sink = FileSink::open(FileSinkConfig {
            path: base.clone(),
            max_bytes: 40,
            max_files: 3,
        })
        .expect("open");
        // Each line is well over 40 bytes after target/message, forcing rotation.
        for i in 0..6u32 {
            sink.write(&rec(format_args!("message-number-{i}")));
        }
        sink.flush();
        // base, base.1, base.2 may exist; base.3 must never exist.
        let mut p3 = base.clone().into_os_string();
        p3.push(".3");
        assert!(!PathBuf::from(p3).exists());
        assert!(base.exists());
    }

    #[test]
    fn file_sink_write_error_increments_dropped_never_panics() {
        // Force a rotation I/O error: with `max_files == 2`, rotation first tries
        // to drop the oldest slot via `remove_file(base.1)`. Pre-create `base.1`
        // as a non-empty directory so `remove_file` fails (it is not a file) on
        // every platform and regardless of privilege. The sink must count the
        // drop and never panic; rotation never deletes a directory by accident.
        let root = std::env::temp_dir().join(format!("spawn_dbg_werr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create dir");
        let base = root.join("rot.log");

        let mut blocker = base.clone().into_os_string();
        blocker.push(".1");
        let blocker = PathBuf::from(blocker);
        std::fs::create_dir(&blocker).expect("create blocker dir");
        std::fs::write(blocker.join("keep"), b"x").expect("populate blocker");

        let sink = FileSink::open(FileSinkConfig {
            path: base.clone(),
            max_bytes: 20,
            max_files: 2,
        })
        .expect("open");
        // Seed the file (size 0 -> no rotation attempt).
        sink.write(&rec(format_args!("first-line-seed")));
        // These exceed max_bytes and attempt rotation, which fails at the
        // `remove_file(base.1)` step. Each is dropped and counted; none panic.
        for i in 0..3u32 {
            sink.write(&rec(format_args!("overflowing-line-{i}")));
        }
        sink.flush();

        let dropped = sink.dropped.load(Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            dropped >= 1,
            "expected at least one dropped record from failed rotation"
        );
    }

    #[test]
    fn file_sink_no_rotation_when_max_files_zero() {
        let dir = std::env::temp_dir();
        let base = dir.join(format!("spawn_dbg_norot_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&base);
        let sink = FileSink::open(FileSinkConfig {
            path: base.clone(),
            max_bytes: 10,
            max_files: 0,
        })
        .expect("open");
        for i in 0..5u32 {
            sink.write(&rec(format_args!("line-{i}")));
        }
        sink.flush();
        let mut p1 = base.clone().into_os_string();
        p1.push(".1");
        assert!(!PathBuf::from(p1).exists());
    }
}
