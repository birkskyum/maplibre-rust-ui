# Exact

`maplibre-exact` is the map in [Exact](https://github.com/expo/exact) apps, drawn by mln-rs on Exact's
GPU canvas. It runs on macOS, iOS and Android.

![Hello Map in Exact on Android, Europe and then Zürich with a dropped pin](../images/exact.webp)

An app draws a map on a canvas:

```text
resource map = exactSurface("maplibre") as shape MapRecord
resource content = mapContent(chosen.id) as shape Content

canvas surface=maplibre(style="https://tiles.openfreemap.org/styles/liberty", longitude=8.54, latitude=47.37, zoom=11, bearing=0, pitch=0, content=content.json)
```

- `content` is the app's sources and layers in the
  [MapLibre Style Specification](https://maplibre.org/maplibre-style-spec/)'s terms, which the data
  module builds (`{"sources": {…}, "layers": […], "images": {…}}`), with images from the app's
  `assets/`. The map applies what changed, new GeoJSON data and changed paint and layout properties
  in place and the rest by adding and removing layers and sources. The content can also set the
  style's `projection` (a globe), `sky`, `light` and `terrain` over the base style's, and values for
  its `global-state` expressions in `state`.
- `options` limits the camera and turns gestures off, with GL JS's `MapOptions` names (`minZoom`,
  `maxZoom`, `minPitch`, `maxPitch`, `maxBounds`, `interactive`, `dragRotate`, `pitchWithRotate`,
  `touchPitch`, `scrollZoom`), and its `transformRequest` rules rewrite the map's requests and add
  headers to them, such as a tile server's token.
- The map takes the mouse with its modifier keys, a trackpad's scroll and pinch, and touches, with GL
  JS's gestures.
- The map posts the events `events` lists (`load`, `moveend`, `click`, `contextmenu`, `error`) to
  the canvas's `message=` action, as GL JS's `map.on` subscribes to them. `contextmenu` is a
  secondary click or a long press.
- Its record, `exactSurface("maplibre")`, holds the camera at rest and what the map shows from it,
  whether everything in view has loaded, the last click and context menu with the content's features under them (a cluster's with
  the zoom it splits at), the last failure, and the style's attribution as text for the app to show
  on the map. The app reads it as a shape of its own.
- Commands go in as JSON (`jumpTo`, `easeTo`, `flyTo`, `fitBounds`, `setStyle`), such as
  ``postMessage(`{"command":"flyTo","center":[8.54,47.37],"zoom":14}`, "maplibre")``, with GL JS's
  camera options, `padding` included.

The names follow MapLibre GL JS.
[`protocol.rs`](https://github.com/birkskyum/maplibre-rust-ui/blob/main/exact/package/rust/src/protocol.rs) binds
the canvas's arguments, [the map model](../model.md) is the rest of the contract, and
[`examples/hello-map`](https://github.com/birkskyum/maplibre-rust-ui/tree/main/exact/examples/hello-map)
uses all of it.

## Using it

The map is an Exact package, [`exact/package`](https://github.com/birkskyum/maplibre-rust-ui/tree/main/exact/package),
whose `exact.json` declares the `maplibre` surface. It is on npm as
[`maplibre-exact`](https://www.npmjs.com/package/maplibre-exact). An app on Exact `d69b9614f` or later
adds it and places the canvas, with no Rust of its own:

```sh
bun exact.mjs add maplibre-exact@0.1.0
```

Exact takes a package from npm by its name and version, and one in a checkout by its folder
(`bun exact.mjs add ../maplibre-rust-ui/exact/package`).

Exact builds the package's crate into a GPU module of its own. The crate builds on Exact's
`exact-gpu` 0.2 and takes `maplibre-rust-ui` from crates.io, and on Android gives mln-rs the app's
Java VM and context, which it fetches the style and tiles through.

## Markers

`markers` in the content holds the canvas's children at places on the map, by their `testId`, as GL
JS's `Marker` holds an element. A marker is the app's own Contract, so it keeps its taps and its
accessibility, and Exact reports it where the map put it. Markers turn and tilt with the map as GL
JS's do. One without a place stays where the app laid it out and turns with the map's bearing, as a
compass does.

```text
// in the content: "markers": {"callout": {"lngLat": [8.5402, 47.3779], "anchor": "bottom"}, "compass": {"rotationAlignment": "map"}}
canvas surface=maplibre(…) testId="map"
  button press=resetNorth aria-label="Reset bearing to north" testId="compass" position="absolute" top=10 right=10
    image "assets/compass.png" width=24 height=24 aria-hidden=true
  column position="absolute" testId="callout"
    text chosenName font-weight=600
    button "Fly here" press=flyChosen
```

## Running the example

Clone Exact next to this repository, on the commit this was last built on, `d69b9614f`:

```sh
git clone https://github.com/expo/exact && git -C exact checkout -b maplibre-exact d69b9614f
bun exact/scripts/exact.mjs setup
```

Then, from `maplibre-rust-ui/exact/examples/hello-map`:

```sh
bun install --frozen-lockfile      # the map's package and Exact's geolocation module
bun exact.mjs mac --run
bun exact.mjs ios --run            # an iOS Simulator; --device on a connected iPhone, with a development profile for its bundle id
bun exact.mjs android --run        # on a connected Android phone
```

[Design](../design.md#what-exact-would-need-to-change) has what Exact would need to change.

## Not done yet

- The web, which Exact builds for too. Exact's GPU module runs on the page's thread with the page's
  WebGPU device, while mln-rs's browser build runs its map in a Web Worker on an `OffscreenCanvas`
  ([Design](../design.md#what-mln-rs-would-need)).
- Linux and Windows, which Exact builds for too, but which haven't been tried.
- A trackpad's rotation, which Exact doesn't pass to a surface yet.
