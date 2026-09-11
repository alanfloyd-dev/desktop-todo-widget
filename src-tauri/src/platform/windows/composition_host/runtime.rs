//! Windows App Runtime (Windows App SDK) lifetime for the composition host.
//!
//! Desktop Acrylic is activated through the **self-contained** Windows App SDK
//! with Undocked RegFree WinRT. Two rules drive this module:
//!
//! 1. The runtime must be loaded *before any window or WebView exists*.
//!    The composition factory runs on the UI thread inside Wry's WebView
//!    creation call stack; calling `LoadLibraryW` from there deadlocks on the
//!    Windows loader lock.
//! 2. It must be loaded exactly once per process.

use std::{
    ffi::c_void,
    sync::atomic::{AtomicIsize, Ordering},
};

use windows::{
    core::{s, w, HRESULT},
    Win32::{
        Foundation::{FreeLibrary, FARPROC, HMODULE},
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
    },
};

/// Handle of the preloaded `Microsoft.WindowsAppRuntime.dll`, or 0 when the
/// runtime has not been loaded yet.
static PRELOADED_MODULE: AtomicIsize = AtomicIsize::new(0);

type EnsureIsLoaded = unsafe extern "system" fn() -> HRESULT;

/// Owns the loaded runtime module and releases the reference on drop.
pub(crate) struct RuntimeLifetime {
    pub(crate) module: usize,
}

impl Drop for RuntimeLifetime {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(HMODULE(self.module as *mut c_void));
        }
        eprintln!("[native-material] shutdown=runtime-released");
    }
}

/// Loads and initializes the self-contained Windows App Runtime.
///
/// Call this from `run()` before the Tauri builder creates any window. The
/// resulting module handle is remembered so [`load_if_needed`] never loads a DLL
/// from inside the WebView creation stack.
pub(crate) fn preload() -> Result<(), String> {
    let runtime = unsafe { load() }?;
    let module = runtime.module;
    PRELOADED_MODULE.store(module as isize, Ordering::Release);
    // The runtime stays loaded for the whole process; the OS reclaims it on exit.
    std::mem::forget(runtime);
    eprintln!("[phase7c2] windows_app_runtime_preloaded=true module={module:#x}");
    Ok(())
}

/// Returns an owned runtime reference only when nothing was preloaded.
///
/// The caller (the composition host context) keeps the returned guard for the
/// lifetime of the window. When [`preload`] already ran, this returns `None` and
/// the process-wide reference stays in place.
pub(crate) fn load_if_needed() -> Result<Option<RuntimeLifetime>, String> {
    match PRELOADED_MODULE.load(Ordering::Acquire) {
        0 => Ok(Some(unsafe { load() }?)),
        _ => Ok(None),
    }
}

/// Loads `Microsoft.WindowsAppRuntime.dll` and calls its
/// `WindowsAppRuntime_EnsureIsLoaded` entry point.
///
/// # Safety
/// Performs process-wide runtime initialization; call from the UI thread before
/// any window or WebView is created.
unsafe fn load() -> Result<RuntimeLifetime, String> {
    let module = LoadLibraryW(w!("Microsoft.WindowsAppRuntime.dll"))
        .map_err(|error| format!("load self-contained Windows App Runtime: {error}"))?;
    let address: FARPROC = GetProcAddress(module, s!("WindowsAppRuntime_EnsureIsLoaded"));
    let Some(address) = address else {
        let _ = FreeLibrary(module);
        return Err("WindowsAppRuntime_EnsureIsLoaded export is unavailable".into());
    };
    let ensure_is_loaded: EnsureIsLoaded = std::mem::transmute_copy(&address);
    if let Err(error) = ensure_is_loaded().ok() {
        let _ = FreeLibrary(module);
        return Err(format!("WindowsAppRuntime_EnsureIsLoaded: {error}"));
    }
    eprintln!("[native-material] runtime_mode=self-contained runtime_loaded=true");
    Ok(RuntimeLifetime {
        module: module.0 as usize,
    })
}
