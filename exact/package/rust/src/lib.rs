//! MapLibre maps in Exact apps, drawn by mln-rs on Exact's GPU canvas.
//!
//! An app draws a map with `canvas surface=maplibre(style=…, …)` and talks to
//! it in JSON: commands go in through Contract's `postMessage`, and events
//! come back through the canvas's `message=` action. [`protocol`] binds the
//! canvas's arguments to the map maplibre-rust-ui models, and [`mln`] is the
//! surface, which draws the map on every host with a GPU canvas.
//!
//! This crate is the Rust half of the `maplibre-exact` package, whose
//! `exact.json` declares the surface. An app adds the package with
//! `bun exact.mjs add <the package>`, and Exact builds the crate into a GPU
//! module of its own.

pub mod mln;
pub mod protocol;

// Gives mln-rs the app's Java VM and application context on Android, which it fetches HTTP and
// reads the app's assets through. Exact generates the package's module with `module!` alone, so
// the entry is defined here, and the module exports it.
exact_gpu::android_init!(|android| unsafe { android_init(android) });

/// # Safety
///
/// `android` is what Exact gives `android_init!`: the calling thread's `JNIEnv*` and a live
/// reference to the application `Context`.
#[cfg(target_os = "android")]
unsafe fn android_init(android: exact_gpu::android::Android) -> Result<(), String> {
    let env = std::ptr::NonNull::new(android.env).ok_or("no JNIEnv")?;
    let context = std::ptr::NonNull::new(android.context).ok_or("no application Context")?;
    unsafe { maplibre_rust_ui::component::android_init(env, context) }
}
