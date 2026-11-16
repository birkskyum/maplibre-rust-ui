//! The camera that the gesture recognizer moves: it reads the map's latest
//! snapshot, and sends each change to the map as a command, as a host of
//! maplibre-native-ffi's C API does.

use std::sync::{Arc, Mutex};

use mln_gestures::{GestureCamera, PendingEnd};
use mln_map::{CameraDelta, CameraOptions, CameraUpdate, MapProjection, TransformState, TransitionEnd};
use mln_util::LatLng;
use mln_util::geo::{ScreenCoordinate, WrapMode};

pub(super) struct SnapshotCamera<'a> {
    pub(super) map: &'a mln_runtime::Map,
    pub(super) state: &'a TransformState,
    pub(super) projection: &'a MapProjection,
}

impl GestureCamera for SnapshotCamera<'_> {
    fn viewport(&self) -> ScreenCoordinate {
        let size = self.state.get_size();
        ScreenCoordinate::new(f64::from(size.width), f64::from(size.height))
    }

    fn camera(&self) -> CameraOptions {
        self.state.get_camera_options(None)
    }

    fn zoom_range(&self) -> (f64, f64) {
        mln_gestures::zoom_range(self.state, mln_gestures::center_latitude(self.state))
    }

    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng> {
        self.projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).ok()
    }

    fn apply_camera_delta(&mut self, delta: &CameraDelta) {
        let _ = self.map.apply_camera_delta(delta.clone());
    }

    fn release(&mut self, delta: &CameraDelta) -> Box<dyn PendingEnd> {
        let end = Arc::new(Mutex::new(None));
        let ended = Arc::clone(&end);
        let _ = self.map.apply_camera_delta_then(delta.clone(), move |result| {
            if let Ok(mut slot) = ended.lock() {
                *slot = result.ok();
            }
        });
        Box::new(EndSlot(end))
    }

    fn update_camera(&mut self, update: &CameraUpdate) {
        let _ = self.map.update_camera(update.clone());
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        let _ = self.map.cancel_camera_transitions_with_id(transition_id);
    }
}

/// A released gesture's end, which the map's thread leaves here.
#[derive(Debug)]
struct EndSlot(Arc<Mutex<Option<TransitionEnd>>>);

impl PendingEnd for EndSlot {
    fn take(&mut self) -> Option<TransitionEnd> {
        self.0.lock().ok()?.take()
    }
}
