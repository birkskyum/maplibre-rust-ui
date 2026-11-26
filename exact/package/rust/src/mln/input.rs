//! Exact's canvas input as maplibre-rust-ui's pointers and the gestures'
//! input.

use exact_gpu::{InputEvent, PointerKind as ExactPointer, PointerPhase};
use maplibre_rust_ui::geo::ScreenCoordinate;
use maplibre_rust_ui::gestures::{Input, Modes, PointerKind};
use maplibre_rust_ui::{Phase, Pointer};

/// The pointer that `event` reports, if it reports one.
pub(super) fn pointer(event: &InputEvent) -> Option<Pointer> {
    let InputEvent::Pointer { id, phase, x, y, kind, buttons, at_ms, .. } = event else { return None };
    Some(Pointer {
        id: *id,
        phase: match phase {
            PointerPhase::Down => Phase::Down,
            PointerPhase::Move => Phase::Move,
            PointerPhase::Up => Phase::Up,
            PointerPhase::Cancel => Phase::Cancel,
        },
        x: f64::from(*x),
        y: f64::from(*y),
        touch: *kind != ExactPointer::Mouse,
        secondary: buttons & 2 != 0,
        at_ms: *at_ms,
    })
}

/// The recognizer's input for `event`, each with when it happened in the
/// host's milliseconds. `held` are the pointers handed down and not yet up,
/// which a blur or Escape cancels, and `center` the map's middle, where keys
/// zoom. A trackpad pinch comes as a wheel with Control held, which zooms at
/// the pinch's rate. Exact reports no modifier keys with a key, so the keys
/// move the map only as they do alone.
pub(super) fn translate(event: &InputEvent, held: &mut Vec<u32>, center: ScreenCoordinate) -> Vec<(Input, f64)> {
    match event {
        InputEvent::Pointer { id, phase, x, y, kind, buttons, shift_key, ctrl_key, at_ms, .. } => {
            let position = ScreenCoordinate::new(f64::from(*x), f64::from(*y));
            let id = *id;
            let input = match phase {
                PointerPhase::Down => {
                    let Some(modes) = modes(*kind, *buttons, *shift_key, *ctrl_key) else { return Vec::new() };
                    held.push(id);
                    Input::PointerDown { id, kind: pointer_kind(*kind), position, modes }
                }
                PointerPhase::Move => Input::PointerMove { id, position },
                PointerPhase::Up => {
                    held.retain(|other| *other != id);
                    Input::PointerUp { id, position }
                }
                PointerPhase::Cancel => {
                    held.retain(|other| *other != id);
                    Input::PointerCancel { id }
                }
            };
            vec![(input, *at_ms)]
        }
        // The web's sign: down is positive, zooming out, 1/300 level a
        // pixel, or 1/100 for a pinch.
        InputEvent::Wheel { dy, x, y, ctrl_key, at_ms, .. } => {
            let rate = if *ctrl_key { 100.0 } else { 300.0 };
            let input = Input::Delta {
                position: ScreenCoordinate::new(f64::from(*x), f64::from(*y)),
                discrete: false,
                pan: ScreenCoordinate::new(0.0, 0.0),
                zoom: -f64::from(*dy) / rate,
                bearing: 0.0,
                pitch: 0.0,
            };
            vec![(input, *at_ms)]
        }
        InputEvent::Key { code, down: true, at_ms, .. } if code == "Escape" => cancel(held, *at_ms),
        InputEvent::Key { code, key, down: true, at_ms, .. } => {
            key_delta(code, key, center).map(|input| vec![(input, *at_ms)]).unwrap_or_default()
        }
        InputEvent::Blur { at_ms } => {
            let mut inputs = cancel(held, *at_ms);
            inputs.push((Input::Modes(Modes::default()), *at_ms));
            inputs
        }
        _ => Vec::new(),
    }
}

/// Cancels the pointers `held`, as Escape drops a drag or a box zoom.
fn cancel(held: &mut Vec<u32>, at_ms: f64) -> Vec<(Input, f64)> {
    held.drain(..).map(|id| (Input::PointerCancel { id }, at_ms)).collect()
}

/// What a press does, from the web's `buttons` mask and its keys: the
/// primary button drags, the secondary one or Control rotates and tilts, and
/// Shift zooms to a box and makes a double click zoom out. The middle button
/// means nothing to the map.
fn modes(kind: ExactPointer, buttons: u32, shift: bool, ctrl: bool) -> Option<Modes> {
    if kind != ExactPointer::Mouse {
        return Some(Modes::default());
    }
    if buttons & 3 == 0 {
        return None;
    }
    Some(Modes { rotate: buttons & 2 != 0 || ctrl, box_zoom: shift, zoom_out: shift })
}

fn pointer_kind(kind: ExactPointer) -> PointerKind {
    match kind {
        ExactPointer::Mouse => PointerKind::Mouse,
        ExactPointer::Touch => PointerKind::Touch,
        ExactPointer::Pen => PointerKind::Pen,
    }
}

/// GL JS's keys: the arrows pan 100 px, and `+` and `-` zoom a level, each
/// eased.
fn key_delta(code: &str, key: &str, center: ScreenCoordinate) -> Option<Input> {
    let (pan, zoom) = match (code, key) {
        ("ArrowLeft", _) => ((100.0, 0.0), 0.0),
        ("ArrowRight", _) => ((-100.0, 0.0), 0.0),
        ("ArrowUp", _) => ((0.0, 100.0), 0.0),
        ("ArrowDown", _) => ((0.0, -100.0), 0.0),
        (_, "+" | "=") => ((0.0, 0.0), 1.0),
        (_, "-" | "_") => ((0.0, 0.0), -1.0),
        _ => return None,
    };
    let pan = ScreenCoordinate::new(pan.0, pan.1);
    Some(Input::Delta { position: center, discrete: true, pan, zoom, bearing: 0.0, pitch: 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(buttons: u32, shift_key: bool, ctrl_key: bool) -> InputEvent {
        InputEvent::Pointer {
            id: 1,
            phase: PointerPhase::Down,
            x: 10.0,
            y: 20.0,
            dx: 0.0,
            dy: 0.0,
            kind: ExactPointer::Mouse,
            buttons,
            shift_key,
            ctrl_key,
            alt_key: false,
            meta_key: false,
            at_ms: 0.0,
        }
    }

    #[test]
    fn a_pinch_is_a_wheel_with_control_and_zooms_faster() {
        let wheel = |ctrl_key| InputEvent::Wheel {
            dx: 0.0,
            dy: -12.0,
            x: 40.0,
            y: 30.0,
            shift_key: false,
            ctrl_key,
            alt_key: false,
            meta_key: false,
            at_ms: 5.0,
        };
        let zoom = |event: &InputEvent| match translate(event, &mut Vec::new(), ScreenCoordinate::new(0.0, 0.0))[..] {
            [(Input::Delta { zoom, discrete: false, .. }, 5.0)] => zoom,
            ref other => panic!("no delta: {other:?}"),
        };
        assert!(zoom(&wheel(false)) > 0.0);
        assert!(zoom(&wheel(true)) > zoom(&wheel(false)));
    }

    #[test]
    fn a_press_takes_its_modes_from_its_buttons_and_keys() {
        let modes = |event: &InputEvent| match translate(event, &mut Vec::new(), ScreenCoordinate::new(0.0, 0.0))[..] {
            [(Input::PointerDown { modes, .. }, _)] => Some(modes),
            _ => None,
        };
        assert_eq!(modes(&press(1, false, false)), Some(Modes::default()));
        assert_eq!(modes(&press(2, false, false)), Some(Modes { rotate: true, ..Modes::default() }));
        assert_eq!(modes(&press(1, false, true)), Some(Modes { rotate: true, ..Modes::default() }));
        assert_eq!(modes(&press(1, true, false)), Some(Modes { box_zoom: true, zoom_out: true, rotate: false }));
        assert_eq!(modes(&press(4, false, false)), None);
    }

    #[test]
    fn a_blur_cancels_the_pointers_down() {
        let mut held = Vec::new();
        let _down = translate(&press(1, false, false), &mut held, ScreenCoordinate::new(0.0, 0.0));
        let inputs = translate(&InputEvent::Blur { at_ms: 9.0 }, &mut held, ScreenCoordinate::new(0.0, 0.0));
        assert!(matches!(inputs[..], [(Input::PointerCancel { id: 1 }, 9.0), (Input::Modes(_), 9.0)]));
        assert!(held.is_empty());
    }
}
