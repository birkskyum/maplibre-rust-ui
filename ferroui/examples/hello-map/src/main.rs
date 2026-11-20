//! Hello Map in a window: `cargo run -p hello-map-ferroui`.

use ferroui_view::prelude::*;

fn main() {
    launch(|| {
        window().title("Hello Map").width(900.0).height(640.0).child(hello_map_ferroui::HelloMap::new().view()).into()
    });
}
