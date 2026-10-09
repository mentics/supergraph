//! Opt-in wall-clock breakdown of where an analysis run spends its time.
//!
//! Disabled by default; when disabled every helper is a plain call. Stages run on the main
//! thread record wall time. `add_cpu` accumulates time summed across worker threads, which is
//! reported separately because it can exceed the wall time of the stage that contains it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static STAGES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

struct Entry {
    name: &'static str,
    total: Duration,
    cpu: bool,
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Runs `work`, recording its wall time under `name` when timing is enabled.
pub fn stage<R>(name: &'static str, work: impl FnOnce() -> R) -> R {
    if !enabled() {
        return work();
    }
    let started = Instant::now();
    let result = work();
    let elapsed = started.elapsed();
    record(name, elapsed, false);
    if std::env::var_os("SUPERGRAPH_TIMINGS_LIVE").is_some() {
        eprintln!("[timing] {name}: {:.1} ms", elapsed.as_secs_f64() * 1000.0);
    }
    result
}

/// Adds time spent inside `work` to a CPU-time counter that is summed across threads.
pub fn add_cpu<R>(name: &'static str, work: impl FnOnce() -> R) -> R {
    if !enabled() {
        return work();
    }
    let started = Instant::now();
    let result = work();
    record(name, started.elapsed(), true);
    result
}

fn record(name: &'static str, elapsed: Duration, cpu: bool) {
    let mut stages = STAGES.lock().unwrap();
    if let Some(entry) = stages
        .iter_mut()
        .find(|entry| entry.name == name && entry.cpu == cpu)
    {
        entry.total += elapsed;
    } else {
        stages.push(Entry {
            name,
            total: elapsed,
            cpu,
        });
    }
}

/// Renders the recorded stages in execution order, with each wall stage as a share of the
/// summed wall time.
pub fn report() -> String {
    let stages = STAGES.lock().unwrap();
    let wall_total = stages
        .iter()
        .filter(|entry| !entry.cpu)
        .map(|entry| entry.total)
        .sum::<Duration>();
    let mut out = String::new();
    out.push_str("stage                                         ms      share\n");
    for entry in stages.iter().filter(|entry| !entry.cpu) {
        out.push_str(&format!(
            "{:<38}{:>10.1}   {:>5.1}%\n",
            entry.name,
            entry.total.as_secs_f64() * 1000.0,
            entry.total.as_secs_f64() / wall_total.as_secs_f64().max(f64::EPSILON) * 100.0
        ));
    }
    out.push_str(&format!(
        "{:<38}{:>10.1}\n",
        "total (sum of stages)",
        wall_total.as_secs_f64() * 1000.0
    ));
    if stages.iter().any(|entry| entry.cpu) {
        out.push_str("\nthread-summed time inside the stages above\n");
        for entry in stages.iter().filter(|entry| entry.cpu) {
            out.push_str(&format!(
                "{:<38}{:>10.1}\n",
                entry.name,
                entry.total.as_secs_f64() * 1000.0
            ));
        }
    }
    out
}
