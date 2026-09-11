//! Desktop Acrylic material backend.
//!
//! Owns the `DesktopAcrylicController`, its `SystemBackdropConfiguration`, the
//! DWM host-backdrop flag, and the single `CompositionTarget` the controller is
//! attached to. When the composition host already created a target, the backend
//! attaches to that shared target so Acrylic and the WebView visual live in one
//! tree; otherwise it creates its own.
//!
//! Shutdown order matters and is deliberately explicit: remove the backdrop
//! target, remove the state subscription, close the controller, then release the
//! Composition objects, and finally clear the DWM host-backdrop flag.

use std::{ffi::c_void, mem::size_of};

use windows::{
    core::{Interface, BOOL},
    UI::Composition::{CompositionTarget, Compositor, ContainerVisual, Desktop::DesktopWindowTarget},
    Win32::{
        Foundation::HWND,
        Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH},
        System::WinRT::Composition::ICompositorDesktopInterop,
    },
};

use crate::winappsdk::Microsoft::UI::{
    Composition::SystemBackdrops::{
        DesktopAcrylicController, SystemBackdropConfiguration, SystemBackdropState,
        SystemBackdropTheme,
    },
    WindowId,
};

/// Desktop Acrylic support probe. Kept as a free function so the capability
/// snapshot has exactly one definition.
pub(crate) fn desktop_acrylic_supported() -> Result<bool, String> {
    DesktopAcrylicController::IsSupported()
        .map_err(|error| format!("DesktopAcrylicController.IsSupported: {error}"))
}

pub(crate) fn state_name(state: SystemBackdropState) -> &'static str {
    if state == SystemBackdropState::Active {
        "active"
    } else if state == SystemBackdropState::Fallback {
        "fallback"
    } else if state == SystemBackdropState::HighContrast {
        "high-contrast"
    } else {
        "unknown"
    }
}

/// Clears the DWM host-backdrop flag if setup fails after arming it.
///
/// Stores the window as a raw value so this guard stays `Send`.
struct HostBackdropGuard {
    hwnd: usize,
    armed: bool,
}

impl Drop for HostBackdropGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = unsafe { set_host_backdrop(HWND(self.hwnd as *mut c_void), false) };
        }
    }
}

pub(crate) struct MaterialBackend {
    controller: DesktopAcrylicController,
    configuration: SystemBackdropConfiguration,
    state_changed_token: Option<i64>,
    window_id: Option<WindowId>,
    root: Option<ContainerVisual>,
    composition_target: Option<CompositionTarget>,
    target: Option<DesktopWindowTarget>,
    compositor: Option<Compositor>,
    hwnd: usize,
    generation: u64,
    target_attached: bool,
    released: bool,
}

impl MaterialBackend {
    /// Creates the backend on `hwnd`, attaching to `shared_target` when the
    /// composition host already owns one.
    ///
    /// # Safety
    /// Must run on the window's owning thread with a live DispatcherQueue.
    pub(crate) unsafe fn new(
        hwnd: HWND,
        generation: u64,
        shared_target: Option<CompositionTarget>,
    ) -> Result<Self, String> {
        set_host_backdrop(hwnd, true)?;
        let mut host_backdrop_guard = HostBackdropGuard {
            hwnd: hwnd.0 as usize,
            armed: true,
        };

        let (compositor, target, root, composition_target) = match shared_target {
            Some(target) => (None, None, None, target),
            None => {
                let compositor =
                    Compositor::new().map_err(|error| format!("Compositor: {error}"))?;
                let interop: ICompositorDesktopInterop = compositor
                    .cast()
                    .map_err(|error| format!("ICompositorDesktopInterop: {error}"))?;
                let target = interop
                    .CreateDesktopWindowTarget(hwnd, true)
                    .map_err(|error| format!("CreateDesktopWindowTarget: {error}"))?;
                let root = compositor
                    .CreateContainerVisual()
                    .map_err(|error| format!("CreateContainerVisual: {error}"))?;
                target
                    .SetRoot(&root)
                    .map_err(|error| format!("DesktopWindowTarget.SetRoot: {error}"))?;
                let composition_target: CompositionTarget = target
                    .cast()
                    .map_err(|error| format!("CompositionTarget projection: {error}"))?;
                (
                    Some(compositor),
                    Some(target),
                    Some(root),
                    composition_target,
                )
            }
        };

        let configuration = SystemBackdropConfiguration::new()
            .map_err(|error| format!("SystemBackdropConfiguration: {error}"))?;
        configuration
            .SetIsInputActive(true)
            .and_then(|_| configuration.SetTheme(SystemBackdropTheme::Dark))
            .map_err(|error| format!("SystemBackdropConfiguration setup: {error}"))?;

        let controller = DesktopAcrylicController::new()
            .map_err(|error| format!("DesktopAcrylicController: {error}"))?;
        controller
            .SetSystemBackdropConfiguration(&configuration)
            .map_err(|error| format!("SetSystemBackdropConfiguration: {error}"))?;

        let window_id = WindowId {
            Value: hwnd.0 as usize as u64,
        };
        let attached = controller
            .SetTargetWithWindowId(window_id, &composition_target)
            .map_err(|error| format!("DesktopAcrylicController.SetTarget: {error}"))?;
        if !attached {
            return Err("DesktopAcrylicController.SetTarget returned false".into());
        }

        let callback_controller = controller.clone();
        let token = controller
            .StateChanged(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                let state = callback_controller.State()?;
                eprintln!(
                    "[native-material] lifecycle_event=StateChanged controller_state={} controller_generation={generation}",
                    state_name(state)
                );
                Ok(())
            }))
            .map_err(|error| format!("DesktopAcrylicController.StateChanged: {error}"))?;

        eprintln!(
            "[native-material] composition_target=DesktopWindowTarget top_level=true root_visual=ContainerVisual controller_generation={generation} set_target=true"
        );
        host_backdrop_guard.armed = false;
        Ok(Self {
            controller,
            configuration,
            state_changed_token: Some(token),
            window_id: Some(window_id),
            root,
            composition_target: Some(composition_target),
            target,
            compositor,
            hwnd: hwnd.0 as usize,
            generation,
            target_attached: true,
            released: false,
        })
    }

    pub(crate) fn is_target_attached(&self) -> bool {
        self.target_attached
    }

    pub(crate) fn state(&self) -> &'static str {
        self.controller
            .State()
            .map(state_name)
            .unwrap_or("unavailable")
    }

    /// Reports the acrylic input-active state used by the lifecycle hook.
    pub(crate) fn set_input_active(&self, active: bool) {
        match self.configuration.SetIsInputActive(active) {
            Ok(()) => eprintln!(
                "[native-material] lifecycle_event=Focused input_active={active} controller_generation={}",
                self.generation
            ),
            Err(error) => eprintln!(
                "[native-material] activation_update=failed HRESULT={:#010x}",
                error.code().0 as u32
            ),
        }
    }

    /// Releases every native object exactly once, in the proven order.
    pub(crate) fn release(&mut self) {
        if self.released {
            return;
        }
        if self.target_attached {
            match self.controller.RemoveAllSystemBackdropTargets() {
                Ok(()) => eprintln!("[native-material] shutdown=targets-removed"),
                Err(error) => eprintln!(
                    "[native-material] shutdown=remove-targets-failed HRESULT={:#010x}",
                    error.code().0 as u32
                ),
            }
            self.target_attached = false;
        }
        if let Some(token) = self.state_changed_token.take() {
            match self.controller.RemoveStateChanged(token) {
                Ok(()) => eprintln!("[native-material] shutdown=subscription-removed"),
                Err(error) => eprintln!(
                    "[native-material] shutdown=remove-subscription-failed HRESULT={:#010x}",
                    error.code().0 as u32
                ),
            }
        }
        match self.controller.Close() {
            Ok(()) => eprintln!("[native-material] shutdown=controller-closed"),
            Err(error) => eprintln!(
                "[native-material] shutdown=controller-close-failed HRESULT={:#010x}",
                error.code().0 as u32
            ),
        }
        self.window_id.take();
        self.root.take();
        self.composition_target.take();
        if let Some(target) = self.target.take() {
            let _ = target.Close();
        }
        self.compositor.take();
        let _ = unsafe { set_host_backdrop(HWND(self.hwnd as *mut c_void), false) };
        self.released = true;
        eprintln!("[native-material] shutdown=composition-released");
    }
}

impl Drop for MaterialBackend {
    fn drop(&mut self) {
        self.release();
    }
}

/// Reads back the DWM host-backdrop flag for diagnostics.
pub(crate) unsafe fn host_backdrop_enabled(hwnd: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
    let mut value = BOOL(0);
    DwmGetWindowAttribute(
        hwnd,
        DWMWA_USE_HOSTBACKDROPBRUSH,
        &mut value as *mut _ as *mut c_void,
        size_of::<BOOL>() as u32,
    )
    .is_ok_and(|_| value.as_bool())
}

pub(crate) unsafe fn set_host_backdrop(hwnd: HWND, enabled: bool) -> Result<(), String> {
    let value = BOOL::from(enabled);
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_HOSTBACKDROPBRUSH,
        &value as *const _ as *const c_void,
        size_of::<BOOL>() as u32,
    )
    .map_err(|error| format!("DWMWA_USE_HOSTBACKDROPBRUSH: {error}"))
}
