//! Temporary `e2etest::*` probe functions the suite registers on its own
//! engine connection so a scenario can present a dependency that lies,
//! disappears, fails transiently, or deduplicates redelivery.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use iii_sdk::errors::Error;
use iii_sdk::iii::FunctionRef;
use iii_sdk::RegisterFunction;
use serde_json::{Map, Value};

use crate::context::E2eContext;

type Ledger = Arc<Mutex<Map<String, Value>>>;

static FUNCTIONS: LazyLock<Mutex<HashMap<String, FunctionRef>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static COUNTERS: LazyLock<Mutex<HashMap<String, Arc<AtomicU32>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static LEDGERS: LazyLock<Mutex<HashMap<String, Ledger>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(in crate::scenarios) fn register<F, Fut>(
    context: &E2eContext,
    id: impl Into<String>,
    description: impl Into<String>,
    handler: F,
) where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Error>> + Send + 'static,
{
    let id = id.into();
    let reference = context.client().register_function(
        id.clone(),
        RegisterFunction::new_async(handler).description(description.into()),
    );
    lock(&FUNCTIONS).insert(id, reference);
}

/// Remove one probe from the engine while the run is still in flight — the
/// registration-loss shape a worker reconnect produces in production.
pub(in crate::scenarios) fn retire(id: &str) {
    if let Some(reference) = lock(&FUNCTIONS).remove(id) {
        reference.unregister();
    }
}

/// Unregister every probe a run installed and drop its recorded call state.
pub(in crate::scenarios) fn release(run_id: &str) {
    let marker = format!("_{}", run_suffix(run_id));
    let retired: Vec<String> = lock(&FUNCTIONS)
        .keys()
        .filter(|id| id.ends_with(&marker))
        .cloned()
        .collect();
    for id in &retired {
        retire(id);
    }
    lock(&COUNTERS).retain(|id, _| !id.ends_with(&marker));
    lock(&LEDGERS).retain(|id, _| !id.ends_with(&marker));
}

pub(in crate::scenarios) fn counter(id: &str) -> Arc<AtomicU32> {
    Arc::clone(
        lock(&COUNTERS)
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(AtomicU32::new(0))),
    )
}

pub(in crate::scenarios) fn hits(id: &str) -> u32 {
    counter(id).load(Ordering::SeqCst)
}

pub(in crate::scenarios) fn record_hit(id: &str) -> u32 {
    counter(id).fetch_add(1, Ordering::SeqCst) + 1
}

pub(in crate::scenarios) fn ledger(id: &str) -> Ledger {
    Arc::clone(
        lock(&LEDGERS)
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(Map::new()))),
    )
}

pub(in crate::scenarios) fn ledger_value(id: &str, key: &str) -> Option<Value> {
    lock(&ledger(id)).get(key).cloned()
}

pub(in crate::scenarios) fn ledger_u64(id: &str, key: &str) -> u64 {
    ledger_value(id, key)
        .as_ref()
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub(in crate::scenarios) fn handler_error(message: impl Into<String>) -> Error {
    Error::Handler(message.into())
}

pub(in crate::scenarios) fn id(namespace: &str, run_id: &str) -> String {
    format!("e2etest::{namespace}_{}", run_suffix(run_id))
}

fn run_suffix(run_id: &str) -> String {
    super::validation_loop::suffix(run_id)
}

/// Cleanup hook body shared by every probe-backed scenario.
pub(in crate::scenarios) fn cleanup<'a>(
    _context: &'a E2eContext,
    run_id: &'a str,
) -> super::CleanupFuture<'a> {
    Box::pin(async move {
        release(run_id);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_ledgers_are_scoped_per_function_id() {
        let first = id("counted", "run-aaaa");
        let second = id("counted", "run-bbbb");
        lock(&ledger(&first)).insert("total".into(), Value::from(5));

        assert_eq!(record_hit(&first), 1);
        assert_eq!(record_hit(&first), 2);
        assert_eq!(hits(&second), 0);
        assert_eq!(ledger_u64(&first, "total"), 5);
        assert_eq!(ledger_u64(&second, "total"), 0);
    }

    #[test]
    fn releasing_a_run_clears_only_that_run() {
        let kept = id("kept", "runc");
        let dropped = id("dropped", "rund");
        record_hit(&kept);
        record_hit(&dropped);

        release("rund");

        assert_eq!(hits(&kept), 1);
        assert_eq!(hits(&dropped), 0);
    }
}
