//! Takes Hello Map for a walk in a window out of sight. The map loads, flies
//! to Zürich, a tap in the middle names Zürich HB, which flying there puts
//! there, the wheel zooms in, the places fit in view and the globe turns on.
//! It saves the window as FerroUI's compositor draws it at each step, in
//! `target/tmp/hello-map-walk`. The map fetches its style and tiles, so the
//! walk only runs when asked, with
//! `cargo test -p hello-map-ferroui --test walk -- --ignored`.
//!
//! The window has no frame, so macOS leaves it where it is put, far off
//! every screen, and it never activates the app, which has no Dock icon. The
//! walk ends at once if the window is on a screen after all.

use std::cell::RefCell;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use ferroui_base::input::raw::{IRawInputEventArgs, RawMouseWheelEventArgs, RawPointerEventArgs, RawPointerEventType};
use ferroui_base::input::{IInputDevice, MouseDevice, RawInputModifiers};
use ferroui_base::media::imaging::{BitmapEncoderOptions, PngBitmapEncoderOptions};
use ferroui_base::rendering::composition::ElementComposition;
use ferroui_base::threading::{Dispatcher, DispatcherPriority, DispatcherTimer};
use ferroui_base::{PixelPoint, Point, Ref, Vector, Visual};
use ferroui_controls::{AppBuilder, Application, Window, WindowDecorations, WindowStartupLocation};
use ferroui_desktop::AppBuilderDesktopExtensions;
use ferroui_native::MacOSPlatformOptions;
use ferroui_themes_fluent::FluentTheme;
use ferroui_view::prelude::*;
use hello_map_ferroui::HelloMap;

/// Ends the process if the walk hangs.
const WALK_TIMEOUT: Duration = Duration::from_secs(120);

fn main() {
    if !std::env::args().any(|arg| arg == "--ignored") {
        println!("walk: ignored, as it fetches the style and tiles from the network (pass --ignored)");
        return;
    }
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hello-map-walk");
    std::fs::create_dir_all(&folder).expect("the folder can be made");
    let args: Vec<String> = Vec::new();
    AppBuilder::configure::<Application>()
        .use_platform_detect()
        .with(Rc::new(MacOSPlatformOptions { show_in_dock: false, ..MacOSPlatformOptions::default() }))
        .start(
            move |app, _| {
                app.styles().add(FluentTheme::new().as_style());
                let made = Rc::new(RefCell::new(None));
                let window = mount_window({
                    let made = made.clone();
                    move || {
                        let hello = HelloMap::new();
                        let view = hello.view();
                        *made.borrow_mut() = Some(hello);
                        window().width(900.0).height(640.0).child(view).into()
                    }
                });
                window.set_window_decorations(WindowDecorations::None);
                window.set_show_activated(false);
                window.set_show_in_taskbar(false);
                window.set_window_startup_location(WindowStartupLocation::Manual);
                window.set_position(PixelPoint::new(-30000, -30000));
                let hello = made.borrow_mut().take().expect("the app is made");
                Dispatcher::ui_thread().spawn_local(walk(window.clone(), hello, folder));
                let _watchdog = DispatcherTimer::run_once(
                    || fail("the walk took too long"),
                    WALK_TIMEOUT,
                    DispatcherPriority::NORMAL,
                )
                .detach();
                app.run_window(window);
            },
            &args,
        );
}

async fn walk(window: Ref<Window>, hello: Rc<HelloMap>, folder: PathBuf) {
    if let Some(screen) = window.screens().screen_from_window(&window) {
        fail(&format!("the window is on a screen, at {:?}", screen.bounds()));
    }
    // Before it shows, FerroUI's macOS window takes no more room than its
    // screen has, which a window off every screen hasn't, so it sizes now.
    window.set_width(900.0);
    window.set_height(640.0);
    wait(Duration::from_secs(8)).await;
    save(&window, &hello, &folder, "1-loaded").await;
    if let Some(error) = hello.map.error() {
        fail(&error);
    }
    hello.fly_to_zurich();
    wait(Duration::from_secs(4)).await;
    save(&window, &hello, &folder, "2-zurich").await;
    // The mouse, as FerroUI's macOS view hands the window its events. The map
    // fills the window's top, so its middle is the window's point there.
    let mouse = Mouse { window: window.clone(), device: MouseDevice::new() };
    let size = hello.map.bounds().size();
    let middle = Point::new(size.width / 2.0, size.height / 2.0);
    mouse.raise(RawPointerEventType::LeftButtonDown, middle);
    mouse.raise(RawPointerEventType::LeftButtonUp, middle);
    wait(Duration::from_secs(2)).await;
    save(&window, &hello, &folder, "3-tapped").await;
    if hello.status.get_untracked() != "Zürich HB" {
        fail(&format!("the tap named {:?}, not Zürich HB", hello.status.get_untracked()));
    }
    let before = zoom(&hello);
    mouse.wheel(middle, Vector::new(0.0, 4.0));
    wait(Duration::from_secs(2)).await;
    save(&window, &hello, &folder, "4-wheel").await;
    if zoom(&hello) <= before {
        fail(&format!("the wheel left the map at zoom {}", zoom(&hello)));
    }
    hello.fit_places();
    wait(Duration::from_secs(3)).await;
    save(&window, &hello, &folder, "5-places").await;
    hello.toggle_globe();
    wait(Duration::from_secs(3)).await;
    save(&window, &hello, &folder, "6-globe").await;
    println!("walk: done");
    window.close();
}

/// The map's zoom at rest.
fn zoom(hello: &HelloMap) -> f64 {
    hello.map.record().map_or(0.0, |record| record.camera.zoom)
}

/// A mouse of the walk's own, which raises events in the window as its
/// platform does.
struct Mouse {
    window: Ref<Window>,
    device: Rc<MouseDevice>,
}

impl Mouse {
    fn send(&self, event: Rc<dyn IRawInputEventArgs>) {
        let input = self.window.platform_impl().and_then(|platform| platform.input());
        let Some(input) = input else { fail("the window takes no input") };
        input(event);
    }

    fn raise(&self, kind: RawPointerEventType, at: Point) {
        let device: Rc<dyn IInputDevice> = self.device.clone();
        let root = self.window.input_root();
        self.send(Rc::new(RawPointerEventArgs::new(device, now(), root, kind, at, RawInputModifiers::empty())));
    }

    /// Scrolls `delta` notches, up being positive.
    fn wheel(&self, at: Point, delta: Vector) {
        let device: Rc<dyn IInputDevice> = self.device.clone();
        let root = self.window.input_root();
        self.send(Rc::new(RawMouseWheelEventArgs::new(device, now(), root, at, delta, RawInputModifiers::empty())));
    }
}

/// Milliseconds since the walk began, as the events' time.
fn now() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

fn fail(why: &str) -> ! {
    eprintln!("walk: {why}");
    std::process::exit(1);
}

/// Writes the window as its compositor draws it to `folder`.
async fn save(window: &Ref<Window>, hello: &HelloMap, folder: &Path, name: &str) {
    let Some(visual) = ElementComposition::get_element_visual(&window.clone().upcast::<Visual>()) else {
        fail("the window has no visual");
    };
    let compositor = visual.compositor().clone();
    let task = compositor.create_composition_visual_snapshot(&visual, window.render_scaling());
    compositor.commit();
    let done = Flag::default();
    let finished = done.clone();
    task.on_completed(move || finished.set());
    done.await;
    let Some(Ok(bitmap)) = task.take_result() else { fail(&format!("the compositor made no snapshot {name}")) };
    let path = folder.join(format!("{name}.png"));
    let png = BitmapEncoderOptions::Png(PngBitmapEncoderOptions::DEFAULT);
    if let Err(error) = bitmap.save_to_file(&path.to_string_lossy(), &png) {
        fail(&format!("{} wasn't written: {error}", path.display()));
    }
    println!("{name}: {} · {}", path.display(), hello.status.get_untracked());
}

/// Waits `time` on the UI thread's clock, while the app runs.
async fn wait(time: Duration) {
    let done = Flag::default();
    let finished = done.clone();
    let _timer = DispatcherTimer::run_once(move || finished.set(), time, DispatcherPriority::NORMAL).detach();
    done.await;
}

/// A future that is ready once it is set.
#[derive(Clone, Default)]
struct Flag(Rc<RefCell<(bool, Option<Waker>)>>);

impl Flag {
    fn set(&self) {
        let mut state = self.0.borrow_mut();
        state.0 = true;
        if let Some(waker) = state.1.take() {
            waker.wake();
        }
    }
}

impl Future for Flag {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.0.borrow_mut();
        if state.0 {
            return Poll::Ready(());
        }
        state.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}
