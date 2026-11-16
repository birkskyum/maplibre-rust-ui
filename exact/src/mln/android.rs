//! The app's Java VM and context, which mln-rs fetches HTTP through on
//! Android. Exact gives a GPU surface neither, so the surface finds the
//! running VM with `JNI_GetCreatedJavaVMs` and the app with
//! `ActivityThread.currentApplication()`, once, before its first map.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Once;

use jni::JavaVM;
use jni::sys::{JNI_OK, jint, jsize};

type GetCreatedJavaVms = unsafe extern "system" fn(*mut *mut jni::sys::JavaVM, jsize, *mut jsize) -> jint;

/// Gives mln-rs the app's Java VM and context, the first time it's called.
pub(super) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = give();
    });
}

fn give() -> Option<()> {
    let vm = running_vm()?;
    let mut env = vm.attach_current_thread().ok()?;
    let app = env
        .call_static_method("android/app/ActivityThread", "currentApplication", "()Landroid/app/Application;", &[])
        .and_then(|app| app.l());
    let Ok(app) = app else {
        let _ = env.exception_clear();
        return None;
    };
    let raw_env = NonNull::new(env.get_raw().cast::<c_void>())?;
    let raw_app = NonNull::new(app.as_raw().cast::<c_void>())?;
    // SAFETY: this thread's env, and the application, alive through the call.
    unsafe { maplibre_rust_ui::component::android_init(raw_env, raw_app) }.ok()
}

/// The process's Java VM, which libnativehelper names since Android 12.
fn running_vm() -> Option<JavaVM> {
    // SAFETY: a NUL-terminated name; the handle stays open for the process.
    let library = unsafe { libc::dlopen(c"libnativehelper.so".as_ptr(), libc::RTLD_NOW) };
    let library = if library.is_null() { libc::RTLD_DEFAULT } else { library };
    // SAFETY: a NUL-terminated name in a loaded library.
    let symbol = unsafe { libc::dlsym(library, c"JNI_GetCreatedJavaVMs".as_ptr()) };
    if symbol.is_null() {
        return None;
    }
    // SAFETY: the symbol is JNI's `JNI_GetCreatedJavaVMs`.
    let get_created: GetCreatedJavaVms = unsafe { std::mem::transmute(symbol) };
    let mut vm = std::ptr::null_mut();
    let mut count: jsize = 0;
    // SAFETY: room for one VM, and its count.
    if unsafe { get_created(&raw mut vm, 1, &raw mut count) } != JNI_OK || count < 1 {
        return None;
    }
    // SAFETY: the process's live VM.
    unsafe { JavaVM::from_raw(vm) }.ok()
}
