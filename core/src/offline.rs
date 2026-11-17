//! The app's offline regions, in the cache database that its maps share, as
//! MapLibre Compose's offline storage keeps its packs: a region is a style's
//! resources for an area and a zoom range, which maps then show without a
//! network. The app names each region, and the name is kept in the region's
//! metadata.

use mln_runtime::{Completion, EventMask, Runtime};
use mln_storage::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionStatus,
    OfflineTilePyramidRegionDefinition,
};
use mln_util::geo::{LatLng, LatLngBounds};
use serde_json::Value as Json;

use crate::model::{Event, Region, RegionState};

/// What the runtime reported of a region's download.
pub(crate) enum Update {
    Status(i64, OfflineRegionStatus),
    Failed(i64, String),
}

/// How long the app waits at least between two `offline` events while a
/// region downloads, in the host's milliseconds.
const POST_INTERVAL_MS: f64 = 250.0;

struct Known {
    id: i64,
    region: Region,
}

/// The regions a map knows of, and the runtime's work on them.
pub(crate) struct Regions {
    known: Vec<Known>,
    listing: Option<Completion<Vec<OfflineRegion>>>,
    /// Regions being made, by name, which start to download once made.
    creating: Vec<(String, Completion<OfflineRegion>)>,
    statuses: Vec<(i64, Completion<OfflineRegionStatus>)>,
    deleting: Vec<(i64, Completion<()>)>,
    /// Commands whose failure the app hears of, with what they did.
    sent: Vec<(String, Completion<()>)>,
    /// Whether the regions changed since the last `offline` event, and
    /// whether in a way the app hears of at once.
    changed: bool,
    urgent: bool,
    posted_at: f64,
}

impl Regions {
    /// The regions in `runtime`'s database, on their way.
    pub(crate) fn new(runtime: &Runtime) -> Regions {
        Regions {
            known: Vec::new(),
            listing: runtime.offline().regions().ok(),
            creating: Vec::new(),
            statuses: Vec::new(),
            deleting: Vec::new(),
            sent: Vec::new(),
            changed: false,
            urgent: false,
            posted_at: f64::NEG_INFINITY,
        }
    }

    fn id(&self, name: &str) -> Result<i64, String> {
        self.known
            .iter()
            .find(|known| known.region.name == name)
            .map(|known| known.id)
            .ok_or(format!("no region {name}"))
    }

    /// Downloads `style_url` for `bounds` from `min_zoom` to `max_zoom`, at
    /// `pixel_ratio`, as the region `name`, or resumes the region of that
    /// name.
    pub(crate) fn download(
        &mut self,
        runtime: &Runtime,
        style_url: Option<&str>,
        pixel_ratio: f32,
        name: String,
        bounds: [[f64; 2]; 2],
        zooms: [f64; 2],
    ) -> Result<(), String> {
        if let Ok(id) = self.id(&name) {
            self.set_state(runtime, id, OfflineRegionDownloadState::Active, &name);
            return Ok(());
        }
        if self.creating.iter().any(|(creating, _)| *creating == name) {
            return Ok(());
        }
        let style_url = style_url.ok_or("a region downloads a style by its URL, and the map's style is a document")?;
        let [[west, south], [east, north]] = bounds;
        let hull = |e: mln_runtime::Error| e.message;
        let bounds =
            LatLngBounds::hull(LatLng::new(south, west).map_err(hull)?, LatLng::new(north, east).map_err(hull)?);
        let definition =
            OfflineTilePyramidRegionDefinition::new(style_url, bounds, zooms[0], zooms[1], pixel_ratio, false)
                .map_err(|e| e.message)?;
        let metadata = serde_json::json!({ "name": name }).to_string().into_bytes();
        let made = runtime.offline().create_region(definition.into(), metadata).map_err(|e| e.message)?;
        self.creating.push((name, made));
        Ok(())
    }

    pub(crate) fn pause(&mut self, runtime: &Runtime, name: &str) -> Result<(), String> {
        let id = self.id(name)?;
        self.set_state(runtime, id, OfflineRegionDownloadState::Inactive, name);
        Ok(())
    }

    pub(crate) fn refresh(&mut self, runtime: &Runtime, name: &str) -> Result<(), String> {
        let id = self.id(name)?;
        self.send(format!("refreshRegion {name}"), runtime.offline().invalidate_region(id));
        Ok(())
    }

    pub(crate) fn delete(&mut self, runtime: &Runtime, name: &str) -> Result<(), String> {
        let id = self.id(name)?;
        let deleted = runtime.offline().delete_region(id).map_err(|e| e.message)?;
        self.deleting.push((id, deleted));
        Ok(())
    }

    fn set_state(&mut self, runtime: &Runtime, id: i64, state: OfflineRegionDownloadState, name: &str) {
        self.send(format!("region {name}"), runtime.offline().set_region_download_state(id, state));
        // The download reports its own status as it goes, and a paused one
        // only through a status asked for.
        if let Ok(status) = runtime.offline().region_status(id) {
            self.statuses.push((id, status));
        }
    }

    fn send(&mut self, what: String, command: Result<Completion<()>, mln_runtime::Error>) {
        self.sent.push((what, command.unwrap_or_else(|e| Completion::ready(Err(e)))));
    }

    /// Takes in what the runtime reported of a download.
    pub(crate) fn update(&mut self, update: Update) {
        match update {
            Update::Status(id, status) => self.set_status(id, &status),
            Update::Failed(id, message) => {
                if let Some(known) = self.known.iter_mut().find(|known| known.id == id) {
                    known.region.error = Some(message);
                    self.changed = true;
                    self.urgent = true;
                }
            }
        }
    }

    fn set_status(&mut self, id: i64, status: &OfflineRegionStatus) {
        let Some(known) = self.known.iter_mut().find(|known| known.id == id) else { return };
        let state = state_of(status);
        self.urgent |= state != known.region.state;
        known.region.state = state;
        known.region.completed_resources = status.completed_resource_count;
        known.region.required_resources = status.required_resource_count;
        known.region.completed_bytes = status.completed_resource_size;
        self.changed = true;
    }

    /// Takes in the runtime's answers, posts `offline` to `events` as the
    /// regions change, and returns the failures the app hears of.
    pub(crate) fn poll(&mut self, runtime: &Runtime, now_ms: f64, events: &mut Vec<Event>) -> Vec<String> {
        let mut failures = Vec::new();
        if let Some(listing) = self.listing.take_if(|listing| listing.is_complete()) {
            match listing.wait() {
                Ok(regions) => {
                    for region in regions {
                        self.know(runtime, &region);
                    }
                }
                Err(e) => failures.push(format!("offline regions: {}", e.message)),
            }
        }
        let (made, creating): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.creating).into_iter().partition(|(_, made)| made.is_complete());
        self.creating = creating;
        for (name, made) in made {
            match made.wait() {
                Ok(region) => {
                    self.know(runtime, &region);
                    self.set_state(runtime, region.id, OfflineRegionDownloadState::Active, &name);
                }
                Err(e) => failures.push(format!("downloadRegion {name}: {}", e.message)),
            }
        }
        let (answered, statuses): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.statuses).into_iter().partition(|(_, status)| status.is_complete());
        self.statuses = statuses;
        for (id, status) in answered {
            if let Ok(status) = status.wait() {
                self.set_status(id, &status);
            }
        }
        let (deleted, deleting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.deleting).into_iter().partition(|(_, deleted)| deleted.is_complete());
        self.deleting = deleting;
        for (id, deleted) in deleted {
            match deleted.wait() {
                Ok(()) => {
                    self.known.retain(|known| known.id != id);
                    self.changed = true;
                    self.urgent = true;
                }
                Err(e) => failures.push(format!("deleteRegion: {}", e.message)),
            }
        }
        let (done, sent): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.sent).into_iter().partition(|(_, done)| done.is_complete());
        self.sent = sent;
        for (what, done) in done {
            if let Err(e) = done.wait() {
                failures.push(format!("{what}: {}", e.message));
            }
        }
        if self.changed && (self.urgent || now_ms - self.posted_at >= POST_INTERVAL_MS) {
            events.push(Event::Offline);
            self.changed = false;
            self.urgent = false;
            self.posted_at = now_ms;
        }
        failures
    }

    /// Adds `region` to those known, and follows its download.
    fn know(&mut self, runtime: &Runtime, region: &OfflineRegion) {
        if self.known.iter().any(|known| known.id == region.id) {
            return;
        }
        let definition = &region.definition;
        let bounds = match definition {
            OfflineRegionDefinition::TilePyramid(pyramid) => pyramid.bounds,
            OfflineRegionDefinition::Geometry(_) => LatLngBounds::world(),
        };
        self.known.push(Known {
            id: region.id,
            region: Region {
                name: name_of(region),
                bounds: [[bounds.west(), bounds.south()], [bounds.east(), bounds.north()]],
                min_zoom: definition.min_zoom(),
                max_zoom: definition.max_zoom(),
                state: RegionState::Paused,
                completed_resources: 0,
                required_resources: 0,
                completed_bytes: 0,
                error: None,
            },
        });
        self.changed = true;
        self.urgent = true;
        self.send(
            format!("region {}", name_of(region)),
            runtime.offline().set_region_observed(region.id, EventMask::ALL_RUNTIME_EVENTS),
        );
        if let Ok(status) = runtime.offline().region_status(region.id) {
            self.statuses.push((region.id, status));
        }
    }

    /// The regions, as the record lists them.
    pub(crate) fn record(&self) -> Vec<Region> {
        self.known.iter().map(|known| known.region.clone()).collect()
    }

    /// Whether a region is downloading, or the runtime has yet to answer,
    /// so that the host keeps running frames, which post the progress.
    pub(crate) fn busy(&self) -> bool {
        self.listing.is_some()
            || !self.creating.is_empty()
            || !self.statuses.is_empty()
            || !self.deleting.is_empty()
            || !self.sent.is_empty()
            || self.changed
            || self.known.iter().any(|known| known.region.state == RegionState::Downloading)
    }
}

/// A region's name: the one its metadata keeps, else the metadata as text,
/// as for a region another app made, else its id.
fn name_of(region: &OfflineRegion) -> String {
    match serde_json::from_slice::<Json>(&region.metadata) {
        Ok(Json::Object(metadata)) if metadata.get("name").is_some_and(Json::is_string) => {
            metadata["name"].as_str().unwrap_or_default().to_owned()
        }
        _ if !region.metadata.is_empty() => String::from_utf8_lossy(&region.metadata).into_owned(),
        _ => region.id.to_string(),
    }
}

/// Where a download is. A region whose style hasn't loaded yet knows of no
/// resource it needs, so it is complete only once it knows them all.
fn state_of(status: &OfflineRegionStatus) -> RegionState {
    if status.required_resource_count_is_precise
        && status.required_resource_count > 0
        && status.completed_resource_count >= status.required_resource_count
    {
        RegionState::Complete
    } else if status.download_state == OfflineRegionDownloadState::Active {
        RegionState::Downloading
    } else {
        RegionState::Paused
    }
}
