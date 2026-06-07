//! Atomic, lock-free metrics callable from any thread, registered by name.
//!
//! Counters and gauges are `u64`-valued. Registration takes an internal lock to
//! guard the name->metric map; reads of an existing metric's value are lock-free
//! atomic loads. Metrics are never removed in Phase 1: each is allocated once and
//! stored as a `'static` reference, so a returned `&Counter`/`&Gauge` is valid
//! for the registry's lifetime (and the get-or-create call holds the map lock
//! only for the lookup, not while the caller uses the metric).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// A monotonically-mutated `u64` counter. All operations use `Relaxed` ordering.
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }

    pub fn increment(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}

/// A `u64`-valued gauge supporting saturating signed adjustment.
pub struct Gauge {
    value: AtomicU64,
}

impl Gauge {
    fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }

    pub fn set(&self, v: u64) {
        self.value.store(v, Ordering::Relaxed);
    }

    /// Saturating signed add: clamps at `0` and `u64::MAX`.
    pub fn add(&self, delta: i64) {
        let mut current = self.value.load(Ordering::Relaxed);
        loop {
            let next = if delta >= 0 {
                current.saturating_add(delta as u64)
            } else {
                current.saturating_sub(delta.unsigned_abs())
            };
            match self.value.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}

/// Whether a snapshot entry is a counter or a gauge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Counter,
    Gauge,
}

/// A point-in-time copy of one metric's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricSnapshot {
    pub name: &'static str,
    pub kind: MetricKind,
    pub value: u64,
}

/// A registry of named counters and gauges. Get-or-create by `&'static str`.
pub struct MetricsRegistry {
    inner: Mutex<Registry>,
}

#[derive(Default)]
struct Registry {
    counters: HashMap<&'static str, &'static Counter>,
    gauges: HashMap<&'static str, &'static Gauge>,
}

impl MetricsRegistry {
    // Spec §3.3 sanctions only `new()`; a public `Default` impl is out of spec.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Registry::default()),
        }
    }

    /// Get-or-create a counter; the same name always returns the same instance.
    /// Metrics are never removed, so the `'static` reference outlives the lock.
    pub fn counter(&self, name: &'static str) -> &Counter {
        let mut reg = lock_recover(&self.inner);
        reg.counters
            .entry(name)
            .or_insert_with(|| Box::leak(Box::new(Counter::new())))
    }

    /// Get-or-create a gauge; the same name always returns the same instance.
    pub fn gauge(&self, name: &'static str) -> &Gauge {
        let mut reg = lock_recover(&self.inner);
        reg.gauges
            .entry(name)
            .or_insert_with(|| Box::leak(Box::new(Gauge::new())))
    }

    /// Point-in-time copy of all current values, stably sorted by name.
    pub fn snapshot(&self) -> Vec<MetricSnapshot> {
        let reg = lock_recover(&self.inner);
        let mut out = Vec::with_capacity(reg.counters.len() + reg.gauges.len());
        for (name, c) in &reg.counters {
            out.push(MetricSnapshot {
                name,
                kind: MetricKind::Counter,
                value: c.get(),
            });
        }
        for (name, g) in &reg.gauges {
            out.push(MetricSnapshot {
                name,
                kind: MetricKind::Gauge,
                value: g.get(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(b.name));
        out
    }

    /// Current value of a metric by name, checking counters then gauges.
    pub fn get(&self, name: &str) -> Option<u64> {
        let reg = lock_recover(&self.inner);
        if let Some(c) = reg.counters.get(name) {
            return Some(c.get());
        }
        reg.gauges.get(name).map(|g| g.get())
    }
}

fn lock_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

static GLOBAL: OnceLock<MetricsRegistry> = OnceLock::new();

/// The process-global default registry.
pub fn global() -> &'static MetricsRegistry {
    GLOBAL.get_or_init(MetricsRegistry::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn counter_ops() {
        let r = MetricsRegistry::new();
        let c = r.counter("hits");
        c.increment();
        c.add(4);
        assert_eq!(c.get(), 5);
        c.reset();
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn gauge_saturating() {
        let r = MetricsRegistry::new();
        let g = r.gauge("mem");
        g.set(10);
        g.add(-3);
        assert_eq!(g.get(), 7);
        g.add(-100);
        assert_eq!(g.get(), 0);
        g.set(u64::MAX);
        g.add(5);
        assert_eq!(g.get(), u64::MAX);
    }

    #[test]
    fn same_name_same_instance() {
        let r = MetricsRegistry::new();
        let a = r.counter("x") as *const Counter;
        let b = r.counter("x") as *const Counter;
        assert_eq!(a, b);
    }

    #[test]
    fn snapshot_sorted_and_kinded() {
        let r = MetricsRegistry::new();
        r.counter("beta").add(2);
        r.gauge("alpha").set(9);
        r.counter("gamma").increment();
        let snap = r.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].name, "alpha");
        assert_eq!(snap[0].kind, MetricKind::Gauge);
        assert_eq!(snap[0].value, 9);
        assert_eq!(snap[1].name, "beta");
        assert_eq!(snap[2].name, "gamma");
    }

    #[test]
    fn get_by_name() {
        let r = MetricsRegistry::new();
        r.counter("c").add(7);
        r.gauge("g").set(3);
        assert_eq!(r.get("c"), Some(7));
        assert_eq!(r.get("g"), Some(3));
        assert_eq!(r.get("missing"), None);
    }

    #[test]
    fn concurrent_increment_and_same_instance() {
        let r = Arc::new(MetricsRegistry::new());
        let n = 8;
        let m = 10_000u64;
        let mut handles = Vec::new();
        for _ in 0..n {
            let r = Arc::clone(&r);
            handles.push(std::thread::spawn(move || {
                let c = r.counter("shared");
                for _ in 0..m {
                    c.increment();
                }
            }));
        }
        for h in handles {
            h.join().expect("join");
        }
        assert_eq!(r.counter("shared").get(), n as u64 * m);
    }
}
