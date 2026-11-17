//! MapLibre maps in Exact apps, drawn by mln-rs on Exact's GPU canvas.
//!
//! An app draws a map with `canvas surface=maplibre(style=…, …)` and talks to
//! it in JSON: commands go in through Contract's `postMessage`, and events
//! come back through the canvas's `message=` action. [`protocol`] binds the
//! canvas's arguments to the map maplibre-rust-ui models, and [`mln`] is the
//! surface, which draws the map on every host with a GPU canvas.
//!
//! Exact registers surfaces in the app's own GPU module, so the app names
//! this one there:
//!
//! ```ignore
//! pub static REGISTRY: exact_gpu::Registry = exact_gpu::Registry {
//!     surfaces: &[("maplibre", maplibre_exact::protocol::ARITY, maplibre_exact::mln::surface)],
//!     shaders: &[],
//! };
//! exact_gpu::module!(REGISTRY);
//! exact_gpu::android_init!(|android| unsafe { maplibre_exact::android_init(android) });
//! ```

pub mod mln;
pub mod protocol;

/// Gives mln-rs the app's Java VM and application context on Android, which it fetches HTTP and
/// reads the app's assets through. The app calls it from Exact's `android_init!` beside
/// `module!`, before any map.
///
/// # Safety
///
/// `android` is what Exact gives `android_init!`: the calling thread's `JNIEnv*` and a live
/// reference to the application `Context`.
#[cfg(target_os = "android")]
pub unsafe fn android_init(android: exact_gpu::android::Android) -> Result<(), String> {
    let env = std::ptr::NonNull::new(android.env).ok_or("no JNIEnv")?;
    let context = std::ptr::NonNull::new(android.context).ok_or("no application Context")?;
    unsafe { maplibre_rust_ui::component::android_init(env, context) }
}
