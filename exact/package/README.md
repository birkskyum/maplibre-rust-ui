# maplibre-exact

MapLibre maps in [Exact](https://github.com/expo/exact) apps, drawn by
[mln-rs](https://github.com/maplibre/mln-rs) on Exact's GPU canvas. It runs on macOS, iOS and Android.

An app adds it with Exact's `add` verb, on Exact `6758f7d0c` or later:

```sh
bun exact.mjs add maplibre-exact@0.3.0
```

Then it draws a map on a canvas:

```text
resource map = exactSurface("maplibre") as shape MapRecord
resource content = mapContent(chosen.id) as shape Content

canvas surface=maplibre(style="https://tiles.openfreemap.org/styles/liberty", longitude=8.54, latitude=47.37, zoom=11, bearing=0, pitch=0, content=content.json)
```

Exact builds the package's crate, in `rust`, into a GPU module of the app's, and Cargo fetches
[`maplibre-rust-ui`](https://crates.io/crates/maplibre-rust-ui) and mln-rs from crates.io for it.

[Its page in the docs](https://birkskyum.github.io/maplibre-rust-ui/hosts/exact/) has the canvas's
arguments, the commands, the events and the record, and
[Hello Map](https://github.com/birkskyum/maplibre-rust-ui/tree/main/exact/examples/hello-map) uses
all of it.
