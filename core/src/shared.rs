//! The one runtime that the app's maps share, as mln-rs's runtime is made to
//! hold them: one connection to the cache database, which holds the offline
//! regions too, and one set of request rules. Maps with runtimes of their own
//! would each open the database, and one connection can then wait without end
//! for another's lock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mln_engine::map::MapEvent;
use mln_runtime::{MapId, Runtime, RuntimeEventKind, RuntimeOptions};

use crate::offline::Update;
use crate::requests;

/// Where a map's events go.
pub(crate) type Sink<T> = Arc<Mutex<Vec<T>>>;

/// The runtime, and why its cache isn't on disk when it isn't.
struct Shared {
    runtime: Runtime,
    cache_failure: Option<String>,
}

static SHARED: OnceLock<Result<Shared, String>> = OnceLock::new();

/// Each map's events, by the map, and every map's sink of offline updates,
/// which concern them all.
static MAPS: Mutex<Option<HashMap<MapId, Sink<MapEvent>>>> = Mutex::new(None);
static OFFLINE: Mutex<Vec<(MapId, Sink<Update>)>> = Mutex::new(Vec::new());

/// The app's runtime, made with the first map, and why its cache isn't on
/// disk when it isn't: a cache database the runtime can't open leaves the
/// cache in memory.
pub(crate) fn runtime() -> Result<(Runtime, Option<String>), String> {
    let shared = SHARED.get_or_init(|| {
        let fail = |e: mln_runtime::Error| format!("maplibre: {}", e.message);
        let cached = crate::cache::path().and_then(|cache_path| {
            Runtime::new(&RuntimeOptions { cache_path, ..RuntimeOptions::default() }).map_err(|e| e.message)
        });
        let (runtime, cache_failure) = match cached {
            Ok(runtime) => (runtime, None),
            Err(why) => {
                (Runtime::new(&RuntimeOptions::default()).map_err(fail)?, Some(format!("maplibre: cache: {why}")))
            }
        };
        // The handler takes effect before any map is made, on the runtime's
        // thread, which runs commands in order.
        let _handler = runtime.set_event_handler(dispatch).map_err(fail)?;
        requests::install(&runtime).map_err(|e| format!("maplibre: transformRequest: {}", e.message))?;
        Ok(Shared { runtime, cache_failure })
    });
    match shared {
        Ok(shared) => Ok((shared.runtime.clone(), shared.cache_failure.clone())),
        Err(why) => Err(why.clone()),
    }
}

/// Has the events of the map `id` go to `events`, and offline updates to
/// `offline`, until [`forget`].
pub(crate) fn follow(id: MapId, events: Sink<MapEvent>, offline: Sink<Update>) {
    MAPS.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_default().insert(id, events);
    OFFLINE.lock().unwrap_or_else(PoisonError::into_inner).push((id, offline));
}

pub(crate) fn forget(id: MapId) {
    if let Some(maps) = MAPS.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        maps.remove(&id);
    }
    OFFLINE.lock().unwrap_or_else(PoisonError::into_inner).retain(|(map, _)| *map != id);
}

fn dispatch(batch: &[mln_runtime::RuntimeEvent]) {
    for event in batch {
        let update = match &event.kind {
            RuntimeEventKind::Map(map_event) => {
                let mln_runtime::EventSource::Map { id, .. } = event.source else { continue };
                let sink = MAPS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .and_then(|maps| maps.get(&id).cloned());
                if let Some(sink) = sink {
                    sink.lock().unwrap_or_else(PoisonError::into_inner).push(map_event.clone());
                }
                continue;
            }
            RuntimeEventKind::OfflineRegionStatusChanged { region_id, status } => Update::Status(*region_id, *status),
            RuntimeEventKind::OfflineRegionResponseError { region_id, error } => {
                Update::Failed(*region_id, format!("{:?}: {}", error.reason, error.message))
            }
        };
        let sinks: Vec<_> =
            OFFLINE.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(_, sink)| sink.clone()).collect();
        for sink in sinks {
            sink.lock().unwrap_or_else(PoisonError::into_inner).push(update.clone());
        }
    }
}
