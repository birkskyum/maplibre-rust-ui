//! Hello Map in FerroUI: a map of a few places in Zürich, which a tap names,
//! with buttons that fly there, fit them in view and turn the globe on, and a
//! callout over the station. `tests/walk.rs` takes it for a walk offscreen.

use std::cell::Cell;
use std::rc::Rc;

use ferroui_base::layout::{HorizontalAlignment, Orientation, VerticalAlignment};
use ferroui_base::{Ref, Thickness};
use ferroui_controls::Control;
use ferroui_view::prelude::*;
use maplibre_ferroui::model::{Camera, Command, Content, Event, Inputs, Options};
use maplibre_ferroui::{FerroMap, at_marker};
use serde_json::json;

const STYLE: &str = "https://tiles.openfreemap.org/styles/liberty";

const PLACES: [(&str, [f64; 2]); 5] = [
    ("Grossmünster", [8.5440, 47.3701]),
    ("Fraumünster", [8.5414, 47.3697]),
    ("Zürich HB", [8.5402, 47.3779]),
    ("ETH Zürich", [8.5481, 47.3763]),
    ("Opernhaus", [8.5466, 47.3653]),
];

/// The places as circles with their names, over the base style, and the
/// globe when it is on.
fn content(globe: bool) -> Content {
    let features: Vec<_> = PLACES
        .iter()
        .map(|(name, at)| {
            json!({"type": "Feature", "properties": {"name": name}, "geometry": {"type": "Point", "coordinates": at}})
        })
        .collect();
    let mut content = json!({
        "sources": {"places": {"type": "geojson", "data": {"type": "FeatureCollection", "features": features}}},
        "layers": [
            {"id": "places", "type": "circle", "source": "places", "paint": {
                "circle-radius": 8, "circle-color": "#2f6fed", "circle-stroke-width": 2, "circle-stroke-color": "#ffffff"}},
            {"id": "place-names", "type": "symbol", "source": "places", "layout": {
                "text-field": ["get", "name"], "text-font": ["Noto Sans Bold"], "text-size": 13,
                "text-anchor": "top", "text-offset": [0, 0.9]},
             "paint": {"text-color": "#1d2b4f", "text-halo-color": "#ffffff", "text-halo-width": 1.5}}
        ],
        "hitPadding": 6,
        "markers": {"station": {"lngLat": PLACES[2].1, "anchor": "bottom", "offset": [0, -14]}}
    });
    if globe {
        content["projection"] = json!({"type": "globe"});
    }
    Content::parse(&content.to_string()).expect("the content is well formed")
}

fn command(json: &serde_json::Value) -> Command {
    Command::parse(&json.to_string()).expect("the command is well formed")
}

fn fly_to_zurich() -> Command {
    command(&json!({"command": "flyTo", "center": PLACES[2].1, "zoom": 14, "duration": 2000}))
}

fn fit_places() -> Command {
    let (lngs, lats): (Vec<f64>, Vec<f64>) = PLACES.iter().map(|(_, [lng, lat])| (*lng, *lat)).unzip();
    let min = |xs: &[f64]| xs.iter().copied().fold(f64::MAX, f64::min);
    let max = |xs: &[f64]| xs.iter().copied().fold(f64::MIN, f64::max);
    command(&json!({
        "command": "fitBounds",
        "bounds": [[min(&lngs), min(&lats)], [max(&lngs), max(&lats)]],
        "padding": 60
    }))
}

fn inputs() -> Inputs {
    Inputs {
        style: STYLE.to_owned(),
        camera: Camera { center: [10.45, 51.16], zoom: 4.0, bearing: 0.0, pitch: 0.0 },
        content: content(false),
        options: Options::default(),
        events: vec![Event::Load, Event::Click, Event::Moveend, Event::Error],
    }
}

/// The app, with the map, the place a tap named and the map's attribution.
pub struct HelloMap {
    pub map: Ref<FerroMap>,
    pub status: Signal<String>,
    attribution: Signal<String>,
    globe: Cell<bool>,
}

impl HelloMap {
    /// The app, made in the reactive owner of its window.
    #[must_use]
    pub fn new() -> Rc<HelloMap> {
        let map = FerroMap::with_inputs(inputs());
        let status = signal("Tap a place".to_owned());
        let attribution = signal(String::new());
        map.on_event(move |map, event| {
            let record = map.record();
            match event {
                Event::Click => {
                    let click = record.as_ref().and_then(|record| record.click.as_ref());
                    let name = click.and_then(|click| {
                        let place = click.features.iter().find(|feature| feature.layer == "places")?;
                        place.properties.get("name").and_then(serde_json::Value::as_str).map(str::to_owned)
                    });
                    status.set(name.unwrap_or_else(|| "Tap a place".to_owned()));
                }
                Event::Error => {
                    let failure = record.as_ref().and_then(|record| record.failure.clone());
                    status.set(failure.or_else(|| map.error()).unwrap_or_default());
                }
                _ => {}
            }
            if let Some(record) = record {
                attribution.set_if_changed(record.attribution);
            }
        });
        Rc::new(HelloMap { map, status, attribution, globe: Cell::new(false) })
    }

    pub fn fly_to_zurich(&self) {
        self.map.command(fly_to_zurich());
    }

    pub fn fit_places(&self) {
        self.map.command(fit_places());
    }

    pub fn toggle_globe(&self) {
        self.globe.set(!self.globe.get());
        let mut inputs = self.map.inputs();
        inputs.content = content(self.globe.get());
        self.map.set_inputs(inputs);
    }

    /// The map with the station's callout over it, and a bar of buttons, the
    /// place a tap named and the map's attribution under it.
    pub fn view(self: &Rc<Self>) -> Element {
        let map = self.map.clone();
        let callout = border()
            .background("#ffffff")
            .corner_radius(6.0)
            .padding(Thickness::new(8.0, 4.0, 8.0, 4.0))
            .child(text_block().text("Trains").foreground("#1d2b4f"))
            .with(move |callout| {
                let callout: Ref<Control> = callout.clone().upcast();
                map.on_markers_moved(move |map| {
                    if let Some(place) = map.marker_places().iter().find(|place| place.name == "station") {
                        at_marker(&callout, place);
                    }
                });
            });
        let (fly, fit, globe) = (self.clone(), self.clone(), self.clone());
        let (status, attribution) = (self.status, self.attribution);
        grid()
            .row_definitions("*,Auto,Auto")
            .background("#101012")
            .child(grid().grid_row(0).child(self.map.clone()).child(canvas().child(callout)))
            .child(
                stack_panel()
                    .grid_row(1)
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .margin(Thickness::new(12.0, 12.0, 12.0, 8.0))
                    .child(action("Zürich", move || fly.fly_to_zurich()))
                    .child(action("Places", move || fit.fit_places()))
                    .child(action("Globe", move || globe.toggle_globe()))
                    .child(
                        text_block()
                            .vertical_alignment(VerticalAlignment::Center)
                            .foreground("#ffffff")
                            .text(move || status.get()),
                    ),
            )
            .child(
                text_block()
                    .grid_row(2)
                    .margin(Thickness::new(12.0, 0.0, 12.0, 8.0))
                    .font_size(11.0)
                    .foreground("#9a9aa2")
                    .horizontal_alignment(HorizontalAlignment::Left)
                    .text(move || attribution.get()),
            )
            .into()
    }
}

/// A button of the bar under the map.
fn action(title: &'static str, click: impl Fn() + 'static) -> El<ferroui_controls::Button> {
    button()
        .name(title)
        .background("#2f6fed")
        .foreground("#ffffff")
        .on_click(move |_| click())
        .child(text_block().text(title))
}
