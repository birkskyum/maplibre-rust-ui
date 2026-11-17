# Getting started

## Adding a map to an app

The core and the Slint, egui, Bevy, Native Shell and Whisker hosts are on crates.io. An app adds its
framework's host, which brings the core and mln-rs with it:

```sh
cargo add maplibre-slint    # or maplibre-egui, maplibre-bevy, maplibre-nsh, maplibre-whisker
```

They build with Rust 1.98 or later, as mln-rs does. The Exact host is the npm package
`maplibre-exact`, which an Exact app adds with `bun exact.mjs add maplibre-exact@0.2.0`. The GPUI host
isn't published, since it builds on a changed GPUI, so an app takes it from a checkout of this
repository.

## The checkouts

To work on the crates or run the examples, check out mln-rs next to this repository (`../mln-rs`), at
the release the core depends on, `v0.0.12`. The crates find it there by path, and the core builds and
tests on its own:

```sh
git clone https://github.com/birkskyum/maplibre-rust-ui
git clone https://github.com/louwers/mln-rs && git -C mln-rs checkout -b maplibre-rust-ui v0.0.12
cd maplibre-rust-ui/core && cargo test
```

Each folder pins its toolchain in `rust-toolchain.toml`, Rust 1.99.1, since mln-rs needs 1.98.

## Running a host's example

Each host folder has the same example, Hello Map, and each but Exact's is a Cargo workspace of its
own. It is a map of a
few places in Zürich, which a tap names, with buttons that fly there, fit them in view and turn the
globe on, and a callout over the station. Most hosts also take a short walk on their own, which loads
the map, flies to Zürich, taps Zürich HB and fits the places in view, and saves what it shows.

| Host | Run it | Take the walk |
|---|---|---|
| [Slint](hosts/slint.md) | `cargo run -p hello-map-slint` | `HELLO_MAP_SNAPSHOTS=/tmp/hello-map cargo run -p hello-map-slint` |
| [egui](hosts/egui.md) | `cargo run -p hello-map-egui` | `cargo test -p hello-map-egui -- --ignored` |
| [Bevy](hosts/bevy.md) | `cargo run -p hello-map-bevy` | `HELLO_MAP_SNAPSHOTS=/tmp/hello-map cargo run -p hello-map-bevy` |
| [GPUI](hosts/gpui.md) | `cargo run -p hello-map-gpui` | `cargo test -p hello-map-gpui -- --ignored` |
| [Native Shell](hosts/nsh.md) | `cargo run -p hello-map-nsh` | `HELLO_MAP_SNAPSHOTS=/tmp/hello-map cargo run -p hello-map-nsh` |
| [Whisker](hosts/whisker.md) | `whisker run ios`, `whisker run android` | `HELLO_MAP_WALK=1` in the app's environment |
| [Exact](hosts/exact.md) | `bun exact.mjs mac --run` | |

Run each from its host's folder, such as `maplibre-rust-ui/slint`. Some hosts need more.

- **GPUI** needs Zed next to this repository, at the commit CI names, with the change to GPUI in
  [`gpui/patches/zed`](https://github.com/birkskyum/maplibre-rust-ui/tree/main/gpui/patches/zed)
  applied.
- **Whisker** needs Whisker's CLI.
- **Native Shell** needs its CLI for iOS, which builds the example for the simulator.
- **Exact** needs Exact next to this repository.

Each host's page has the details.

## Declaring a map

Every host takes the same declaration. In Rust it is an `Inputs`, which each host re-exports in its
`model` module, and the hosts whose views are native, Native Shell and Whisker, take the same fields as
JSON props.

```rust
use maplibre_slint::model::{Camera, Command, Content, Event, Inputs, Options};

let inputs = Inputs {
    style: "https://tiles.openfreemap.org/styles/liberty".to_owned(),
    camera: Camera { center: [8.54, 47.37], zoom: 11.0, bearing: 0.0, pitch: 0.0 },
    content: Content::parse(r##"{
        "sources": {"places": {"type": "geojson", "data": "https://example.com/places.geojson"}},
        "layers": [{"id": "places", "type": "circle", "source": "places",
                    "paint": {"circle-radius": 8, "circle-color": "#2f6fed"}}]
    }"##)?,
    options: Options::default(),
    events: vec![Event::Load, Event::Click, Event::Moveend],
};

let fly_to_zurich = Command::parse(r#"{"command": "flyTo", "center": [8.54, 47.37], "zoom": 14}"#)?;
```

[The map model](model.md) has every field, command, event and what the record holds, and the
[API docs](https://birkskyum.github.io/maplibre-rust-ui/api/maplibre_rust_ui/) have the core's types.
