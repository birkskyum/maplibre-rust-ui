//! Where the maps keep their cache database, which holds what they loaded,
//! so that a map loads its style and tiles from the disk the next time, and
//! the offline regions. Every map of the app shares it. By default it is in
//! the app's cache directory, as MapLibre's SDKs keep theirs, and
//! [`set_cache_path`] moves it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What [`set_cache_path`] set: `None` while it wasn't called, and
/// `Some(None)` to keep the cache in memory.
static CHOSEN: Mutex<Option<Option<PathBuf>>> = Mutex::new(None);

/// The app's cache directory on Android, which `android_init` finds.
#[cfg(target_os = "android")]
static ANDROID_CACHE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

const FILE: &str = "maplibre.db";

/// Has the maps made from now on keep their cache database at `path`, or in
/// memory for `None`, instead of in the app's cache directory.
pub fn set_cache_path(path: Option<PathBuf>) {
    *CHOSEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path);
}

/// The cache database for a new map: the one [`set_cache_path`] chose, else
/// a file in the app's cache directory, whose directory this makes. `None`
/// keeps the cache in memory, as in the browser, which caches the requests
/// itself.
pub(crate) fn path() -> Result<Option<PathBuf>, String> {
    let chosen = CHOSEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let path = match chosen {
        Some(path) => path,
        None => default_dir().map(|dir| dir.join(FILE)),
    };
    if let Some(dir) = path.as_deref().and_then(Path::parent) {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't make the cache directory {}: {e}", dir.display()))?;
    }
    Ok(path)
}

/// The app's own directory for caches, with a folder of this library's.
fn default_dir() -> Option<PathBuf> {
    if cfg!(target_family = "wasm") {
        return None;
    }
    #[cfg(target_os = "android")]
    return ANDROID_CACHE.get().map(|dir| dir.join("maplibre"));
    #[cfg(not(target_os = "android"))]
    {
        let base = platform_cache_dir()?;
        Some(base.join(app_name()?).join("maplibre"))
    }
}

/// The user's or, in a sandbox, the app's cache directory.
#[cfg(not(target_os = "android"))]
fn platform_cache_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    if cfg!(target_vendor = "apple") {
        // In an app's sandbox, as on iOS, `HOME` is the app's container.
        var("HOME").map(|home| home.join("Library").join("Caches"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA")
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
    }
}

/// The running program's name, which keeps apps that share a cache
/// directory, as on a Mac outside a sandbox, apart.
#[cfg(not(target_os = "android"))]
fn app_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.file_stem()?.to_string_lossy().into_owned())
}

/// Keeps the cache directory of the Android `context`.
///
/// # Safety
///
/// `jni_env` is the calling thread's `JNIEnv*`, and `context` a live
/// reference to an Android `Context`.
#[cfg(target_os = "android")]
pub(crate) unsafe fn android_init(
    jni_env: *mut std::ffi::c_void,
    context: *mut std::ffi::c_void,
) -> Result<(), String> {
    use jni::objects::{JObject, JString};
    use jni::{EnvUnowned, Outcome, jni_sig, jni_str};

    if ANDROID_CACHE.get().is_some() {
        return Ok(());
    }
    // SAFETY: the caller's contract; the local references made here go with
    // the caller's frame.
    let mut env = unsafe { EnvUnowned::from_raw(jni_env.cast()) };
    let outcome = env
        .with_env(|env| -> Result<String, jni::errors::Error> {
            // SAFETY: the caller's reference, which nothing here deletes.
            let context = unsafe { JObject::from_raw(env, context.cast()) };
            let found = (|| {
                let dir = env.call_method(&context, jni_str!("getCacheDir"), jni_sig!("()Ljava/io/File;"), &[])?.l()?;
                let path =
                    env.call_method(&dir, jni_str!("getAbsolutePath"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
                env.cast_local::<JString>(path)?.try_to_string(env)
            })();
            if env.exception_check() {
                env.exception_clear();
            }
            found
        })
        .into_outcome();
    match outcome {
        Outcome::Ok(path) => {
            let _ = ANDROID_CACHE.set(PathBuf::from(path));
            Ok(())
        }
        Outcome::Err(error) => Err(format!("the app's cache directory: {error}")),
        Outcome::Panic(_) => Err("the app's cache directory: JNI panicked".to_owned()),
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    #[test]
    fn the_cache_is_in_the_apps_own_folder_of_the_cache_directory() {
        let Some(dir) = default_dir() else { return };
        let name = app_name().unwrap_or_default();
        assert!(dir.ends_with(Path::new(&name).join("maplibre")), "{}", dir.display());
        if cfg!(target_vendor = "apple") {
            assert!(dir.to_string_lossy().contains("/Library/Caches/"), "{}", dir.display());
        }
    }
}
