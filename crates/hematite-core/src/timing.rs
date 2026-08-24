//! Phase timing.
//!
//! Answering "why is this slow" by reading gaps between log lines is guesswork: it
//! attributes time to whatever happened to log next, which is rarely the thing that was
//! actually running. These are explicit measurements around named spans, accumulated
//! across a run and reported as a table.
//!
//! Recording is a mutex lock and a duration add, so it is cheap enough to leave in
//! permanently. Nothing is printed unless a caller asks for the report.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Record {
    total: Duration,
    calls: u64,
}

fn table() -> &'static Mutex<HashMap<String, Record>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Record>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Time a span, recording on drop.
///
/// Drop rather than an explicit stop so an early `return` or `?` inside the span still
/// records: a phase that exits by the error path is exactly the one worth seeing.
pub struct Span {
    label: &'static str,
    started: Instant,
}

impl Span {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            started: Instant::now(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        record(self.label, self.started.elapsed());
    }
}

/// Start timing a named span.
pub fn span(label: &'static str) -> Span {
    Span::new(label)
}

/// Add to a span's running total.
pub fn record(label: &str, elapsed: Duration) {
    if let Ok(mut t) = table().lock() {
        let e = t.entry(label.to_string()).or_default();
        e.total += elapsed;
        e.calls += 1;
    }
}

/// Time a closure and return its value.
pub fn measure<T>(label: &'static str, f: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = f();
    record(label, started.elapsed());
    out
}

/// Every recorded span, slowest first, as `(label, total, calls)`.
pub fn report() -> Vec<(String, Duration, u64)> {
    let Ok(t) = table().lock() else {
        return Vec::new();
    };
    let mut rows: Vec<(String, Duration, u64)> = t
        .iter()
        .map(|(k, v)| (k.clone(), v.total, v.calls))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows
}

/// Whether anything was recorded.
pub fn is_empty() -> bool {
    table().lock().map(|t| t.is_empty()).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_accumulate_across_calls() {
        for _ in 0..3 {
            let _s = span("test::accumulate");
            std::thread::sleep(Duration::from_millis(1));
        }
        let rows = report();
        let row = rows
            .iter()
            .find(|(l, _, _)| l == "test::accumulate")
            .expect("recorded");
        assert_eq!(row.2, 3);
        assert!(row.1 >= Duration::from_millis(3));
    }

    /// A span must record even when its scope exits early, since the error path is often
    /// the slow one.
    #[test]
    fn a_span_records_on_early_return() {
        fn bail() -> Option<()> {
            let _s = span("test::early");
            None
        }
        assert!(bail().is_none());
        assert!(report().iter().any(|(l, _, _)| l == "test::early"));
    }
}
