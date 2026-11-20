//! MapLibre maps in [FerroUI](https://github.com/wieslawsoltes/FerroUI) apps,
//! drawn by mln-rs through maplibre-rust-ui's engine into IOSurfaces that
//! FerroUI's compositor draws as they are.
//!
//! A [`FerroMap`] is a control whose content is the map the app declares
//! ([`model::Inputs`]), filling the room its layout gives it:
//!
//! ```ignore
//! let map = FerroMap::with_inputs(inputs);
//! map.on_event(|map, event| {
//!     // a click's features are in map.record()
//! });
//! map.command(fly_to_zurich);
//! ```
//!
//! The map renders on a thread of its own into a ring of IOSurfaces, and the
//! control hands FerroUI's compositor the newest as a frame of a drawing
//! surface, which Skia's Graphite samples on Metal, so nothing is copied. A
//! frame goes back to the map once the compositor's last GPU read of it
//! completed. The control takes the mouse and touches with their modifier
//! keys, the wheel, and a trackpad's pinch and rotation, and asks for frames
//! while the map loads, animates or follows a gesture.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use ferroui_base::input::{
    InputElement, InputElementImpl, InputElementImplExt, KeyModifiers, PointerCaptureLostEventArgs,
    PointerDeltaEventArgs, PointerEventArgs, PointerPressedEventArgs, PointerReleasedEventArgs, PointerType,
    PointerUpdateKind, PointerWheelEventArgs,
};
use ferroui_base::interactivity::InteractiveImpl;
use ferroui_base::layout::LayoutableImpl;
use ferroui_base::media::{Brushes, DrawingContext, IBrush, MediaContext, RotateTransform};
use ferroui_base::platform::{
    KnownPlatformGraphicsExternalImageHandleTypes, LeasedPlatformHandle, PlatformGraphicsExternalImageFormat,
    PlatformGraphicsExternalImageProperties,
};
use ferroui_base::rendering::composition::frames::{
    DeviceIdentity, FrameAlpha, FrameBudget, FrameBudgetLimits, FrameColor, FrameFormat, FrameIdentity, FrameLease,
    FrameMetadata, FramePlane, FrameRegion, FrameRotation, GpuCompletion,
};
use ferroui_base::rendering::composition::{
    CompositionDrawingSurface, CompositionSurfaceVisual, ElementComposition, ICompositionGpuInterop,
};
use ferroui_base::threading::{Dispatcher, DispatcherPriority, DispatcherTimer};
use ferroui_base::{
    FerroObjectImpl, FerroObjectImplExt, FerroPropertyChangedEventArgs, Rect, Ref, Size, StyledElementImpl, Vector,
    Visual, VisualImpl, VisualImplExt, VisualTreeAttachmentEventArgs, ferro_class, ferro_class_info,
    ferro_impl_classes, instantiate,
};
use ferroui_controls::{Canvas, Control, ControlImpl};
use maplibre_rust_ui::component::Component;
pub use maplibre_rust_ui::component::{MarkerPlace, Press};
pub use maplibre_rust_ui::gestures::{Button, Modifiers};
use maplibre_rust_ui::gestures::{Input, PinchPhase, WheelDelta};
pub use maplibre_rust_ui::model;
use maplibre_rust_ui::model::{Command, Event, Inputs, Record};
use maplibre_rust_ui::runtime::{AcquiredFrame, ExportKind, ExportedImage, FrameTextureKind};
use maplibre_rust_ui::util::geo::ScreenCoordinate;
use maplibre_rust_ui::util::size::Size as MapSize;
pub use maplibre_rust_ui::{Phase, Viewport};

/// How long after a trackpad's last pinch or rotation the gesture ends, and
/// the map glides on. The platform says when one goes on, but not when it ends.
const PINCH_ENDS_AFTER: Duration = Duration::from_millis(120);

/// What the app does with an event of the map's.
type EventHandler = Rc<dyn Fn(&FerroMap, Event)>;

/// What the app does when a marker at a place moved.
type MovedHandler = Rc<dyn Fn(&FerroMap)>;

/// A map in a FerroUI app, and what the app knows of it.
#[repr(C)]
pub struct FerroMap {
    base: Control,
    component: RefCell<Component>,
    record: RefCell<Option<Record>>,
    places: RefCell<Vec<MarkerPlace>>,
    /// Why the map can't show, past what the component knows.
    failure: RefCell<Option<String>>,
    events: RefCell<Vec<EventHandler>>,
    moved: RefCell<Vec<MovedHandler>>,
    shown: RefCell<Option<Shown>>,
    /// Counts the times the map left its window, so that a composition asked
    /// for before then isn't taken after.
    attachment: Cell<u64>,
    asking: Cell<bool>,
    /// A frame is on its way to the compositor, and the newest frame the map
    /// finished since, which goes next.
    in_flight: Cell<bool>,
    next: RefCell<Option<AcquiredFrame>>,
    sequence: Cell<u64>,
    frame_asked: Cell<bool>,
    /// The pointers held down on the map, with the button each holds.
    down: RefCell<Vec<(u32, Button)>>,
    /// The trackpad's pinch, as the count of its updates, which a timer
    /// ends once they stop.
    pinch: Cell<u64>,
}

ferro_class!(FerroMap: Control);
ferro_impl_classes!(FerroMap: StyledElementImpl, LayoutableImpl, InteractiveImpl, ControlImpl);
ferro_class_info!(FerroMap { new: FerroMap::new });

/// Where the map's frames go, the drawing surface of a visual that is the
/// control's own, which the compositor draws over what the control draws.
struct Shown {
    interop: Rc<dyn ICompositionGpuInterop>,
    surface: CompositionDrawingSurface,
    visual: CompositionSurfaceVisual,
    budget: FrameBudget,
    /// The map's frames as one source of the compositor's.
    source: u64,
}

impl FerroMap {
    #[must_use]
    pub fn construct() -> FerroMap {
        FerroMap::construct_with(Component::new())
    }

    fn construct_with(component: Component) -> FerroMap {
        FerroMap {
            base: Control::construct(),
            component: RefCell::new(component),
            record: RefCell::new(None),
            places: RefCell::new(Vec::new()),
            failure: RefCell::new(None),
            events: RefCell::new(Vec::new()),
            moved: RefCell::new(Vec::new()),
            shown: RefCell::new(None),
            attachment: Cell::new(0),
            asking: Cell::new(false),
            in_flight: Cell::new(false),
            next: RefCell::new(None),
            sequence: Cell::new(0),
            frame_asked: Cell::new(false),
            down: RefCell::new(Vec::new()),
            pinch: Cell::new(0),
        }
    }

    /// A map of the whole world, made once it has a size and its inputs a
    /// style.
    #[must_use]
    pub fn new() -> Ref<FerroMap> {
        instantiate(FerroMap::construct())
    }

    /// A map as `inputs` declares it, made once it has a size.
    #[must_use]
    pub fn with_inputs(inputs: Inputs) -> Ref<FerroMap> {
        instantiate(FerroMap::construct_with(Component::with_inputs(inputs)))
    }

    /// The map the app declares now. The map takes what changed at its next
    /// frame, and a camera moves only when its value changes.
    pub fn set_inputs(&self, inputs: Inputs) {
        self.component.borrow_mut().set_inputs(inputs);
        self.ask_frame();
    }

    #[must_use]
    pub fn inputs(&self) -> Inputs {
        self.component.borrow().inputs().clone()
    }

    /// Sends the map `command`, which starts at its next frame.
    pub fn command(&self, command: Command) {
        self.component.borrow_mut().send(command);
        self.ask_frame();
    }

    /// Gives the map the image the content names `src`, as an encoded file.
    ///
    /// # Errors
    ///
    /// Why the image didn't decode.
    pub fn add_image(&self, src: &str, bytes: &[u8]) -> Result<(), String> {
        self.ask_frame();
        self.component.borrow_mut().add_image(src, bytes)
    }

    /// The map's record, with its camera at rest, whether everything in view has
    /// loaded, the last click and context menu, the last failure and the
    /// style's attribution.
    #[must_use]
    pub fn record(&self) -> Option<Record> {
        self.record.borrow().clone()
    }

    /// Where the content's markers at places go, on the map as it last drew,
    /// for [`at_marker`].
    #[must_use]
    pub fn marker_places(&self) -> Vec<MarkerPlace> {
        self.places.borrow().clone()
    }

    /// The map's size in points and its pixels a point, once it has a size.
    #[must_use]
    pub fn viewport(&self) -> Option<Viewport> {
        self.component.borrow().viewport()
    }

    /// Why the map couldn't be made or shown, if it couldn't.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        self.component.borrow().refused().map(str::to_owned).or_else(|| self.failure.borrow().clone())
    }

    /// Calls `handler` with each event the inputs ask for, after the map's
    /// record took it in.
    pub fn on_event(&self, handler: impl Fn(&FerroMap, Event) + 'static) {
        self.events.borrow_mut().push(Rc::new(handler));
    }

    /// Calls `handler` when a marker at a place moved, after
    /// [`marker_places`](Self::marker_places) took it in.
    pub fn on_markers_moved(&self, handler: impl Fn(&FerroMap) + 'static) {
        self.moved.borrow_mut().push(Rc::new(handler));
    }

    /// A pointer on the map, at points from its top-left corner, as the
    /// mouse's are, for input of the app's own.
    pub fn pointer(&self, press: &Press) {
        self.component.borrow_mut().pointer(press);
        self.ask_frame();
    }

    fn gesture(&self, input: Input) {
        self.component.borrow_mut().gesture(input);
        self.ask_frame();
    }

    /// Asks for a frame of the map before the window's next.
    fn ask_frame(&self) {
        if self.frame_asked.replace(true) {
            return;
        }
        let map = self.to_ref().downgrade();
        MediaContext::instance().request_animation_frame(move |_| {
            if let Some(map) = map.upgrade() {
                map.frame_asked.set(false);
                map.frame();
            }
        });
    }

    /// Runs the map's frame for the control as it is laid out, and hands the
    /// compositor the map's newest image.
    fn frame(&self) {
        if !self.is_attached_to_visual_tree() {
            return;
        }
        let bounds = self.bounds();
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }
        let Some(top_level) = ferroui_controls::TopLevel::get_top_level(Some(self)) else { return };
        let viewport = Viewport {
            size: MapSize::new(bounds.width.round() as u32, bounds.height.round() as u32),
            scale: top_level.render_scaling() as f32,
        };
        let frame = self.component.borrow_mut().frame_shared(viewport, &[ExportKind::IoSurface]);
        *self.record.borrow_mut() = self.component.borrow().record();
        let places = self.component.borrow().marker_places();
        let moved = places != *self.places.borrow();
        if moved {
            *self.places.borrow_mut() = places;
        }
        if let Some(image) = frame.image {
            self.show(image);
        }
        if frame.again {
            self.ask_frame();
        }
        let handlers = self.events.borrow().clone();
        for post in &frame.posts {
            for handler in &handlers {
                handler(self, post.event);
            }
        }
        if moved {
            for handler in self.moved.borrow().clone() {
                handler(self);
            }
        }
    }

    /// Asks the compositor for its GPU interop, and makes the visual the
    /// map's frames go to, once the control is in a window.
    fn compose(&self) {
        if self.shown.borrow().is_some() || self.asking.get() {
            return;
        }
        let Some(element) = ElementComposition::get_element_visual(&self.to_ref()) else { return };
        let compositor = element.compositor().clone();
        let attachment = self.attachment.get();
        let map = self.to_ref().downgrade();
        self.asking.set(true);
        Dispatcher::ui_thread().spawn_local(async move {
            let interop = compositor.try_get_composition_gpu_interop().await;
            let Some(map) = map.upgrade() else { return };
            map.asking.set(false);
            if map.attachment.get() != attachment || !map.is_attached_to_visual_tree() {
                return;
            }
            let interop = interop.filter(|interop| {
                let surfaces = KnownPlatformGraphicsExternalImageHandleTypes::IO_SURFACE_REF;
                interop.supported_image_handle_types().iter().any(|kind| kind == surfaces)
            });
            let Some(interop) = interop else {
                map.fail("maplibre: FerroUI's renderer here takes no IOSurfaces; it does with Skia on Metal");
                return;
            };
            let surface = compositor.create_drawing_surface();
            let visual = compositor.create_surface_visual();
            visual.set_surface(Some((*surface).clone()));
            let size = map.bounds().size();
            visual.set_size(Vector::new(size.width, size.height));
            ElementComposition::set_element_child_visual(&map, Some((*visual).clone()));
            // The map renders into a ring of three. The compositor shows one,
            // reads another on its way there, and the map renders the third.
            let limits = FrameBudgetLimits { frames: 3, ..FrameBudgetLimits::default() };
            static SOURCES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let source = SOURCES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            *map.shown.borrow_mut() =
                Some(Shown { interop, surface, visual, budget: FrameBudget::new(limits), source });
            map.ask_frame();
        });
    }

    /// Lets go of the compositor's visual, whose frames go back to the map
    /// as the compositor's reads of them end.
    fn uncompose(&self) {
        self.attachment.set(self.attachment.get() + 1);
        self.next.borrow_mut().take();
        self.in_flight.set(false);
        ElementComposition::set_element_child_visual(&self.to_ref(), None);
        if let Some(shown) = self.shown.borrow_mut().take() {
            shown.visual.set_surface(None);
            shown.visual.dispose();
            shown.surface.dispose();
        }
    }

    fn fail(&self, message: &str) {
        *self.failure.borrow_mut() = Some(message.to_owned());
        let handlers = self.events.borrow().clone();
        for handler in &handlers {
            handler(self, Event::Error);
        }
    }

    /// Hands the compositor `frame`, or keeps it for when the frame before
    /// it arrived, in place of an older one, which goes back to the map.
    fn show(&self, frame: AcquiredFrame) {
        if self.in_flight.get() {
            *self.next.borrow_mut() = Some(frame);
            return;
        }
        let leased = {
            let shown = self.shown.borrow();
            let Some(shown) = shown.as_ref() else { return };
            self.lease(shown, frame).map(|lease| self.present(shown, lease))
        };
        if let Err(why) = leased
            && self.failure.borrow().as_deref() != Some(why.as_str())
        {
            self.fail(&why);
        }
    }

    /// Imports `lease`'s IOSurface into the compositor, as the frame of the
    /// map's drawing surface from the compositor's next frame on.
    fn present(&self, shown: &Shown, lease: FrameLease) {
        let metadata = lease.metadata();
        let surface = lease.resource::<MapFrame>().map_or(0, |frame| frame.surface as isize);
        let handle = LeasedPlatformHandle::new(
            surface,
            Some(KnownPlatformGraphicsExternalImageHandleTypes::IO_SURFACE_REF),
            lease.clone(),
        );
        let image = shown.interop.import_image(
            Rc::new(handle),
            PlatformGraphicsExternalImageProperties {
                width: metadata.width as i32,
                height: metadata.height as i32,
                format: PlatformGraphicsExternalImageFormat::B8G8R8A8UNorm,
                top_left_origin: true,
                memory_size: metadata.retained_bytes,
                ..Default::default()
            },
        );
        // The map's GPU work on a frame completed before the map gave it out.
        let ready = GpuCompletion::new();
        ready.complete();
        let task = shown.surface.update_shared_frame_async(&*image, lease, ready);
        image.dispose_async();
        self.in_flight.set(true);
        let map = self.to_ref().downgrade();
        let attachment = self.attachment.get();
        let done = task.clone();
        task.on_completed(move || {
            let Some(map) = map.upgrade() else { return };
            if map.attachment.get() != attachment {
                return;
            }
            map.in_flight.set(false);
            if let Some(error) = done.exception() {
                map.fail(&format!("maplibre: FerroUI's compositor didn't take the map's frame: {error}"));
                return;
            }
            let next = map.next.borrow_mut().take();
            if let Some(next) = next {
                map.show(next);
            }
        });
    }

    /// `frame` as a frame of the compositor's, which holds it until its last
    /// GPU read of it completed.
    ///
    /// # Errors
    ///
    /// Why the compositor can't take it.
    fn lease(&self, shown: &Shown, frame: AcquiredFrame) -> Result<FrameLease, String> {
        let surface = frame.texture().and_then(|texture| match &texture.kind {
            FrameTextureKind::Exported(ExportedImage::IoSurface { surface }) => Some((*surface, texture.size)),
            _ => None,
        });
        let Some((surface, size)) = surface else { return Err("maplibre: the map's frame is no IOSurface".to_owned()) };
        let (width, height) = (size.width, size.height);
        let (stride, bytes) = surface_layout(surface).ok_or("maplibre: the map's frame has no IOSurface")?;
        let sequence = self.sequence.get() + 1;
        self.sequence.set(sequence);
        let metadata = FrameMetadata {
            identity: FrameIdentity { source: shown.source, generation: 1, sequence },
            width,
            height,
            visible: FrameRegion { x: 0, y: 0, width, height },
            format: FrameFormat::Bgra8,
            color: FrameColor::Srgb,
            alpha: FrameAlpha::Opaque,
            rotation: FrameRotation::None,
            mirrored: false,
            top_left_origin: true,
            timestamp: Duration::ZERO,
            // An IOSurface goes to any of the Mac's devices, so the frame
            // names the compositor's, which the import asks for.
            device: shown.interop.device_luid().map(DeviceIdentity::Luid),
            device_generation: shown.interop.shared_context_generation(),
            planes: vec![FramePlane { offset: 0, stride, length: bytes }],
            retained_bytes: bytes,
        };
        shown
            .budget
            .lease(metadata, MapFrame { surface, _frame: frame })
            .map_err(|why| format!("maplibre: FerroUI's compositor takes no more of the map's frames: {why:?}"))
    }

    /// The control's pointer `e` as the map takes it.
    fn press(&self, e: &PointerEventArgs, id: u32, phase: Phase, button: Button) {
        let at = e.get_position(Some(self));
        let touch = e.pointer().type_() == PointerType::Touch;
        let modifiers = modifiers(e.key_modifiers());
        self.pointer(&Press { id, phase, x: at.x, y: at.y, button, touch, modifiers });
    }

    /// A trackpad's pinch or rotation, which goes on until its updates stop.
    fn pinched(&self, e: &PointerDeltaEventArgs, scale: f64, rotation: f64) {
        let at = e.get_position(Some(self));
        let center = ScreenCoordinate::new(at.x, at.y);
        let count = self.pinch.get() + 1;
        self.pinch.set(count);
        self.gesture(Input::Pinch { center, scale, rotation, phase: PinchPhase::Update });
        let map = self.to_ref().downgrade();
        let _ended = DispatcherTimer::run_once(
            move || {
                let Some(map) = map.upgrade() else { return };
                if map.pinch.get() == count {
                    map.gesture(Input::Pinch { center, scale: 1.0, rotation: 0.0, phase: PinchPhase::End });
                }
            },
            PINCH_ENDS_AFTER,
            DispatcherPriority::NORMAL,
        );
    }
}

impl FerroObjectImpl for FerroMap {
    fn constructed(this: &Self) {
        Self::parent_constructed(this);
        this.set_clip_to_bounds(true);
        this.pointer_touch_pad_gesture_magnify(|map, e: &PointerDeltaEventArgs| {
            if let Some(map) = map.to_ref().cast::<FerroMap>() {
                map.pinched(e, 1.0 + e.delta().x, 0.0);
            }
        });
        this.pointer_touch_pad_gesture_rotate(|map, e: &PointerDeltaEventArgs| {
            if let Some(map) = map.to_ref().cast::<FerroMap>() {
                // AppKit turns counterclockwise; the map clockwise.
                map.pinched(e, 1.0, -e.delta().x);
            }
        });
    }

    fn on_property_changed(this: &Self, args: &FerroPropertyChangedEventArgs<'_>) {
        Self::parent_on_property_changed(this, args);
        if args.property() == Visual::bounds_property().as_property() {
            if let Some(shown) = this.shown.borrow().as_ref() {
                let size = this.bounds().size();
                shown.visual.set_size(Vector::new(size.width, size.height));
            }
            this.ask_frame();
        }
    }
}

impl VisualImpl for FerroMap {
    fn on_attached_to_visual_tree(this: &Self, args: &VisualTreeAttachmentEventArgs) {
        Self::parent_on_attached_to_visual_tree(this, args);
        this.compose();
        this.ask_frame();
    }

    fn on_detached_from_visual_tree(this: &Self, args: &VisualTreeAttachmentEventArgs) {
        this.uncompose();
        Self::parent_on_detached_from_visual_tree(this, args);
    }

    /// A transparent fill under the map's visual, which pointers hit.
    fn render(this: &Self, context: &mut DrawingContext) {
        let size = this.bounds().size();
        let clear: Rc<dyn IBrush> = Brushes::transparent();
        context.fill_rectangle(&clear, Rect::new(0.0, 0.0, size.width, size.height), 0.0);
    }
}

impl InputElementImpl for FerroMap {
    fn on_pointer_pressed(this: &Self, e: &PointerPressedEventArgs) {
        Self::parent_on_pointer_pressed(this, e);
        let button = match e.properties().pointer_update_kind {
            PointerUpdateKind::LeftButtonPressed => Button::Primary,
            PointerUpdateKind::RightButtonPressed => Button::Secondary,
            PointerUpdateKind::MiddleButtonPressed => Button::Middle,
            _ => return,
        };
        let id = e.pointer().id() as u32;
        if this.down.borrow().iter().any(|(down, _)| *down == id) {
            return;
        }
        this.down.borrow_mut().push((id, button));
        e.pointer().capture(Some(&this.to_ref().upcast::<InputElement>()));
        e.set_handled(true);
        this.press(e, id, Phase::Down, button);
    }

    fn on_pointer_moved(this: &Self, e: &PointerEventArgs) {
        Self::parent_on_pointer_moved(this, e);
        let id = e.pointer().id() as u32;
        let held = this.down.borrow().iter().find(|(down, _)| *down == id).map(|(_, button)| *button);
        if let Some(button) = held {
            this.press(e, id, Phase::Move, button);
        }
    }

    fn on_pointer_released(this: &Self, e: &PointerReleasedEventArgs) {
        Self::parent_on_pointer_released(this, e);
        let id = e.pointer().id() as u32;
        let held = this.down.borrow().iter().position(|(down, _)| *down == id);
        if let Some(index) = held {
            let (_, button) = this.down.borrow_mut().remove(index);
            this.press(e, id, Phase::Up, button);
            e.pointer().capture(None);
            e.set_handled(true);
        }
    }

    fn on_pointer_capture_lost(this: &Self, e: &PointerCaptureLostEventArgs) {
        Self::parent_on_pointer_capture_lost(this, e);
        let id = e.pointer().id() as u32;
        let held = this.down.borrow().iter().position(|(down, _)| *down == id);
        if let Some(index) = held {
            let (_, button) = this.down.borrow_mut().remove(index);
            let press = Press {
                id,
                phase: Phase::Cancel,
                x: 0.0,
                y: 0.0,
                button,
                touch: false,
                modifiers: Modifiers::default(),
            };
            this.pointer(&press);
        }
    }

    fn on_pointer_wheel_changed(this: &Self, e: &PointerWheelEventArgs) {
        Self::parent_on_pointer_wheel_changed(this, e);
        let at = e.get_position(Some(this));
        // FerroUI's macOS view gives a trackpad's points over 50 and a
        // wheel's lines over 5, up being positive; the map takes the web's
        // pixels, down being positive.
        let delta = WheelDelta::Pixels(-e.delta().y * 50.0);
        let modifiers = modifiers(e.key_modifiers());
        this.gesture(Input::Wheel { position: ScreenCoordinate::new(at.x, at.y), delta, modifiers });
        e.set_handled(true);
    }
}

/// A frame of the map as the compositor's frames hold it, its IOSurface and
/// the frame, which goes back to the map as the last of them drops.
struct MapFrame {
    surface: u64,
    _frame: AcquiredFrame,
}

/// The bytes a row of the IOSurface at `surface` takes, and all its bytes.
fn surface_layout(surface: u64) -> Option<(u64, u64)> {
    let surface = std::ptr::NonNull::new(surface as usize as *mut objc2_io_surface::IOSurfaceRef)?;
    // SAFETY: the frame that names the surface holds it while it lives.
    let surface = unsafe { surface.as_ref() };
    Some((surface.bytes_per_row() as u64, surface.alloc_size() as u64))
}

fn modifiers(keys: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: keys.contains(KeyModifiers::SHIFT),
        ctrl: keys.contains(KeyModifiers::CONTROL),
        alt: keys.contains(KeyModifiers::ALT),
        meta: keys.contains(KeyModifiers::META),
    }
}

/// Holds `child`, a child of a [`Canvas`] the map fills, at a marker's
/// place, with its anchor on the point, turned as the marker turns, and hidden
/// while the place is.
pub fn at_marker(child: &Control, place: &MarkerPlace) {
    child.measure(Size::new(f64::INFINITY, f64::INFINITY));
    let size = child.desired_size();
    let [ax, ay] = place.anchor;
    Canvas::set_left(child, f64::from(place.x) - f64::from(ax) * size.width);
    Canvas::set_top(child, f64::from(place.y) - f64::from(ay) * size.height);
    child.set_render_transform_origin(ferroui_base::RelativePoint::new(
        f64::from(ax),
        f64::from(ay),
        ferroui_base::RelativeUnit::Relative,
    ));
    child.set_render_transform(Some(RotateTransform::with_angle(f64::from(place.rotate)).into()));
    child.set_opacity(if place.hidden { 0.0 } else { 1.0 });
}
