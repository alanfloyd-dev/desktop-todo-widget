//! Window context: the single owner of live HWND truth for the composition path.
//!
//! [`WindowContext`] holds the window handle, the owning thread, the shared
//! composition visual tree, the optional Acrylic backend, the WinRT DispatcherQueue
//! and apartment, and the self-contained runtime reference.
//!
//! [`ContextStore`] is the process-side facade the Wry hooks and the product
//! window layer talk to. It is the only public surface of this module tree: no
//! HWND, COM interface, or composition object crosses it except as an opaque
//! `IUnknown` root visual target that Wry immediately hands to WebView2.

use std::{ffi::c_void, mem::size_of, sync::Mutex};

use windows::{
    core::{IUnknown, Interface, HRESULT},
    System::DispatcherQueueController,
    Win32::{
        Foundation::{HWND, CO_E_NOTINITIALIZED},
        System::{
            Com::{CoGetApartmentType, APTTYPE, APTTYPEQUALIFIER, APTTYPE_MAINSTA, APTTYPE_STA},
            Threading::GetCurrentThreadId,
            WinRT::{
                CreateDispatcherQueueController, DispatcherQueueOptions, RoInitialize,
                RoUninitialize, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, RO_INIT_SINGLETHREADED,
            },
        },
        UI::WindowsAndMessaging::{GetParent, GetWindow, GetWindowThreadProcessId, GW_OWNER},
    },
};

use super::{
    geometry::ClientSize,
    material::{self, PlatformCapabilities, RequestedMaterial, ResolvedMaterial},
    material_backend::{self, MaterialBackend},
    runtime::{self, RuntimeLifetime},
    visual::ComVisual,
    NativeHost,
};

/// Error used for platform-boundary failures; carries the Win32-style
/// "invalid state" HRESULT so callers can log it like a COM error.
pub(crate) fn platform_error(message: impl AsRef<str>) -> windows::core::Error {
    windows::core::Error::new(HRESULT(0x8000_4005_u32 as i32), message.as_ref())
}

/// Releases the WinRT apartment on drop.
struct ApartmentLifetime;

impl Drop for ApartmentLifetime {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
        eprintln!("[native-material] shutdown=winrt-uninitialized");
    }
}

struct WindowContext {
    /// Stored as a raw value, not `HWND`, so the managed-state store keeps its
    /// `Send` bound. Converted to `HWND` at each native call.
    hwnd: usize,
    owner_thread: u32,
    material: Option<MaterialBackend>,
    visual: Option<ComVisual>,
    dispatcher: Option<DispatcherQueueController>,
    apartment: Option<ApartmentLifetime>,
    runtime: Option<RuntimeLifetime>,
    controller_generation: u64,
}

impl WindowContext {
    /// Creates the context for `hwnd` after checking the thread model.
    ///
    /// # Safety
    /// `hwnd` must be a live window owned by the calling thread.
    unsafe fn new(hwnd: HWND) -> Result<Self, String> {
        let owner_thread = GetWindowThreadProcessId(hwnd, None);
        let current_thread = GetCurrentThreadId();
        if owner_thread == 0 || owner_thread != current_thread {
            return Err(format!(
                "unsafe thread model: hwnd_thread={owner_thread} current_thread={current_thread}"
            ));
        }

        let runtime = runtime::load_if_needed()?;

        let mut apartment_type = APTTYPE::default();
        let mut apartment_qualifier = APTTYPEQUALIFIER::default();
        match CoGetApartmentType(&mut apartment_type, &mut apartment_qualifier) {
            Ok(()) => {
                if apartment_type != APTTYPE_STA && apartment_type != APTTYPE_MAINSTA {
                    return Err(format!(
                        "Tauri HWND thread has incompatible COM apartment type {} qualifier {}",
                        apartment_type.0, apartment_qualifier.0
                    ));
                }
            }
            Err(error) if error.code() == CO_E_NOTINITIALIZED => {
                apartment_type = APTTYPE::default();
                apartment_qualifier = APTTYPEQUALIFIER::default();
            }
            Err(error) => return Err(format!("CoGetApartmentType: {error}")),
        }

        RoInitialize(RO_INIT_SINGLETHREADED).map_err(|error| format!("RoInitialize: {error}"))?;
        let apartment = ApartmentLifetime;
        CoGetApartmentType(&mut apartment_type, &mut apartment_qualifier)
            .map_err(|error| format!("CoGetApartmentType after RoInitialize: {error}"))?;

        let dispatcher = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_NONE,
        })
        .map_err(|error| format!("CreateDispatcherQueueController: {error}"))?;

        log_window_facts(hwnd, owner_thread, current_thread);
        eprintln!(
            "[native-material] hwnd_owner_thread={owner_thread} dispatcher_thread={current_thread} compositor_thread={current_thread} lifecycle_thread={current_thread} destruction_thread={owner_thread}"
        );
        eprintln!(
            "[native-material] com_apartment_type={} com_apartment_qualifier={} winrt_initialized_by_context=true",
            apartment_type.0, apartment_qualifier.0
        );

        Ok(Self {
            hwnd: hwnd.0 as usize,
            owner_thread,
            material: None,
            visual: None,
            dispatcher: Some(dispatcher),
            apartment: Some(apartment),
            runtime,
            controller_generation: 0,
        })
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut c_void)
    }

    fn hwnd_value(&self) -> usize {
        self.hwnd
    }

    fn assert_owner_thread(&self) -> Result<(), String> {
        let current = unsafe { GetCurrentThreadId() };
        if current == self.owner_thread {
            Ok(())
        } else {
            Err(format!(
                "native material access rejected: hwnd_thread={} current_thread={current}",
                self.owner_thread
            ))
        }
    }

    fn capabilities(&self) -> Result<PlatformCapabilities, String> {
        material_backend::desktop_acrylic_supported()
            .map(|desktop_acrylic_supported| PlatformCapabilities {
                desktop_acrylic_supported,
            })
    }

    fn next_generation(&mut self) -> u64 {
        self.controller_generation += 1;
        self.controller_generation
    }

    fn disable_material(&mut self) {
        if let Some(mut material) = self.material.take() {
            material.release();
        }
    }

    fn shutdown(&mut self) {
        self.disable_material();
        if let Some(mut visual) = self.visual.take() {
            visual.release();
        }
        self.dispatcher.take();
        eprintln!("[native-material] shutdown=dispatcher-released");
        self.apartment.take();
        self.runtime.take();
    }
}

impl Drop for WindowContext {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Process-side facade over the window context.
///
/// Registered with Wry as the Windows-only composition host factory, and shared
/// with the product window runtime through an `Arc`.
#[derive(Default)]
pub struct ContextStore {
    context: Mutex<Option<WindowContext>>,
}

impl ContextStore {
    /// Wry composition-host factory hook.
    ///
    /// Returns the WebView2 `RootVisualTarget` for the `main` WebView, or `None`
    /// for every other WebView so windowed hosting is untouched.
    pub fn prepare_webview(
        &self,
        id: &str,
        hwnd_value: isize,
    ) -> windows::core::Result<Option<IUnknown>> {
        if id != "main" {
            return Ok(None);
        }
        let hwnd = HWND(hwnd_value as *mut c_void);
        let mut context = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if context.is_none() {
            let created = unsafe { WindowContext::new(hwnd) }.map_err(platform_error)?;
            *context = Some(created);
        }
        let context = context.as_mut().expect("context was initialized");
        context.assert_owner_thread().map_err(platform_error)?;
        if context.hwnd_value() != hwnd.0 as usize {
            return Err(platform_error("composition host HWND changed unexpectedly"));
        }
        if context.visual.is_none() {
            context.visual = Some(unsafe { ComVisual::new(hwnd) }?);
        }
        let visual = context.visual.as_ref().expect("visual was initialized");
        eprintln!(
            "[phase7c2] host_selected=true webview_id={id} hwnd={:#x} root_visual_target_set=pending",
            hwnd.0 as usize
        );
        visual.webview_root_target().map(Some)
    }

    /// Wry resize hook. Sizes the shared visual tree to the client area.
    ///
    /// `width`/`height` are accepted for logging parity with the hook signature
    /// but the client rect is re-read, because it is the single geometry source
    /// of truth and the caller's values can lag a pending resize.
    pub fn resize_webview(
        &self,
        id: &str,
        hwnd_value: isize,
        width: i32,
        height: i32,
    ) -> windows::core::Result<()> {
        if id != "main" {
            return Ok(());
        }
        let mut context = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // The HWND check happens before borrowing the visual mutably.
        let Some(expected_hwnd) = context
            .as_ref()
            .and_then(|context| context.visual.as_ref().map(ComVisual::hwnd))
        else {
            return Ok(());
        };
        if expected_hwnd as isize != hwnd_value {
            return Err(platform_error("composition resize HWND mismatch"));
        }
        let _ = (width, height);
        let context = context.as_mut().expect("context was present");
        let Some(visual) = context.visual.as_mut() else {
            return Ok(());
        };
        visual.resize(ClientSize::of(visual.native_hwnd())?)
    }

    /// Applies the post-WebView controller settings that need a live controller.
    pub fn attach_controller(&self, window: &tauri::WebviewWindow) -> Result<(), String> {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2Controller3,
            COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS,
        };

        let context = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(visual) = context.as_ref().and_then(|context| context.visual.as_ref()) else {
            return Ok(());
        };
        let hwnd = visual.hwnd();

        let outcome = std::sync::Arc::new(Mutex::new(Ok(())));
        let callback_outcome = std::sync::Arc::clone(&outcome);
        window
            .with_webview(move |webview| unsafe {
                let result = (|| -> Result<(), String> {
                    let controller: ICoreWebView2Controller = webview.controller();
                    controller
                        .cast::<ICoreWebView2CompositionController>()
                        .map_err(|error| format!("ICoreWebView2CompositionController: {error}"))?;
                    let controller3: ICoreWebView2Controller3 = controller
                        .cast()
                        .map_err(|error| format!("ICoreWebView2Controller3: {error}"))?;
                    controller3
                        .SetBoundsMode(COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS)
                        .map_err(|error| format!("SetBoundsMode(raw pixels): {error}"))?;
                    controller3
                        .SetShouldDetectMonitorScaleChanges(false)
                        .map_err(|error| format!("SetShouldDetectMonitorScaleChanges: {error}"))?;
                    let scale = super::geometry::DpiScale::of(HWND(hwnd as *mut c_void));
                    controller3
                        .SetRasterizationScale(scale.scale)
                        .map_err(|error| format!("SetRasterizationScale: {error}"))?;
                    Ok(())
                })();
                *callback_outcome
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = result;
            })
            .map_err(|error| error.to_string())?;
        let callback_result = outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        callback_result?;
        eprintln!(
            "[phase7c2] webview_composition_controller_created=true root_visual_target_set=true input_bridge=mouse-wheel-focus"
        );
        Ok(())
    }

    pub fn diagnostic_summary(&self) -> String {
        let context = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(context) = context.as_ref() else {
            return "desktop_window_target_created=false set_target=false controller_state=detached"
                .into();
        };
        let Some(material) = context.material.as_ref() else {
            return "desktop_window_target_created=false set_target=false controller_state=none"
                .into();
        };
        format!(
            "desktop_window_target_created={} root_visual_retained={} set_target={} controller_state={}",
            context.visual.is_some(),
            context.visual.is_some(),
            material.is_target_attached(),
            material.state()
        )
    }

    /// Resolves and applies the native material for the current host.
    pub fn apply_material(
        &self,
        window: &tauri::WebviewWindow,
        requested_glass: bool,
        host: NativeHost,
    ) -> Result<(), String> {
        let mut context = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let requested = if requested_glass {
            RequestedMaterial::Glass
        } else {
            RequestedMaterial::Other
        };

        // Only an expanded floating Glass window may create a DesktopAcrylic
        // controller; every other combination keeps the existing product material.
        if !(requested_glass && host == NativeHost::FloatingExpanded) {
            if let Some(context) = context.as_mut() {
                context.assert_owner_thread()?;
                context.disable_material();
            }
            let resolved = material::resolve(
                requested,
                host,
                PlatformCapabilities {
                    desktop_acrylic_supported: false,
                },
            );
            eprintln!(
                "[native-material] requested_material={} resolved_material={resolved:?} native_backend=none window_mode={host:?}",
                if requested_glass { "Glass" } else { "non-Acrylic" },
            );
            return Ok(());
        }

        let window_hwnd = window
            .hwnd()
            .map(|value| value.0 as usize)
            .map_err(|error| error.to_string())?;
        transparent_webview(window)?;
        if let Some(current) = context.as_ref() {
            if current.hwnd_value() != window_hwnd {
                return Err(
                    "native material context encountered an unexpected HWND replacement".into(),
                );
            }
        }
        let hwnd = context
            .as_ref()
            .map_or(HWND(window_hwnd as *mut c_void), WindowContext::hwnd);
        if context.is_none() {
            match unsafe { WindowContext::new(hwnd) } {
                Ok(created) => *context = Some(created),
                Err(error) => {
                    eprintln!(
                        "[native-material] requested_material=Glass resolved_material=GlassFallback native_backend=none desktop_acrylic_supported=false controller_state=unavailable set_target=false runtime_mode=self-contained window_mode=Floating reason={error}"
                    );
                    return Ok(());
                }
            }
        }

        let context = context.as_mut().expect("context was initialized");
        context.assert_owner_thread()?;
        let capabilities = context.capabilities()?;
        let resolved = material::resolve(
            RequestedMaterial::Glass,
            NativeHost::FloatingExpanded,
            capabilities,
        );
        if resolved != ResolvedMaterial::DesktopAcrylic {
            context.disable_material();
            eprintln!(
                "[native-material] requested_material=Glass resolved_material=GlassFallback native_backend=none desktop_acrylic_supported=false controller_state=unsupported set_target=false runtime_mode=self-contained window_mode=Floating"
            );
            return Ok(());
        }

        if context.material.is_none() {
            let shared_target = context
                .visual
                .as_ref()
                .map(|visual| visual.composition_target());
            let generation = context.next_generation();
            match unsafe { MaterialBackend::new(hwnd, generation, shared_target) } {
                Ok(material) => context.material = Some(material),
                Err(error) => {
                    context.disable_material();
                    eprintln!(
                        "[native-material] requested_material=Glass resolved_material=GlassFallback native_backend=none desktop_acrylic_supported=true controller_state=failed set_target=false runtime_mode=self-contained window_mode=Floating reason={error}"
                    );
                    return Ok(());
                }
            }
        }

        let material = context.material.as_ref().expect("material was initialized");
        eprintln!(
            "[native-material] requested_material=Glass resolved_material=DesktopAcrylic native_backend=DesktopAcrylicController desktop_acrylic_supported=true controller_state={} set_target={} runtime_mode=self-contained window_mode=Floating",
            material.state(),
            material.is_target_attached()
        );
        Ok(())
    }

    /// Tracks window activation for the Acrylic active/fallback state machine.
    pub fn set_input_active(&self, active: bool) {
        let mut context = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(context) = context.as_mut() else {
            return;
        };
        if let Err(error) = context.assert_owner_thread() {
            eprintln!("[native-material] activation_update=skipped reason={error}");
            return;
        }
        if let Some(material) = context.material.as_ref() {
            material.set_input_active(active);
        }
    }

    /// Releases the native material. When the composition host owns the visual
    /// tree, the host itself is released later, after Wry detaches the
    /// `RootVisualTarget`.
    pub fn shutdown(&self) {
        let mut context = self
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(context_value) = context.as_mut() else {
            return;
        };
        if let Err(error) = context_value.assert_owner_thread() {
            eprintln!("[native-material] shutdown=wrong-thread reason={error}");
            return;
        }
        if context_value.visual.is_some() {
            context_value.disable_material();
            eprintln!("[phase7c2] lifecycle=host-release-deferred-until-wry-drop");
            return;
        }
        let mut context_value = context.take().expect("context was present");
        context_value.shutdown();
    }
}

/// Makes the WebView2 background transparent so the composition tree below it
/// can show through. The composition backend is the single owner of this
/// setting.
fn transparent_webview(window: &tauri::WebviewWindow) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
    };

    let outcome = std::sync::Arc::new(Mutex::new(Ok(())));
    let callback_outcome = std::sync::Arc::clone(&outcome);
    window
        .with_webview(move |webview| unsafe {
            let result = (|| -> Result<(), String> {
                let controller: ICoreWebView2Controller2 = webview
                    .controller()
                    .cast()
                    .map_err(|error| format!("ICoreWebView2Controller2: {error}"))?;
                let mut before = COREWEBVIEW2_COLOR::default();
                let _ = controller.DefaultBackgroundColor(&mut before);
                controller
                    .SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                        A: 0,
                        R: 0,
                        G: 0,
                        B: 0,
                    })
                    .map_err(|error| format!("SetDefaultBackgroundColor: {error}"))?;
                eprintln!(
                    "[native-material] webview_default_background_before=ARGB({},{},{},{}) webview_default_background_after=transparent",
                    before.A, before.R, before.G, before.B
                );
                Ok(())
            })();
            *callback_outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = result;
        })
        .map_err(|error| error.to_string())?;
    let callback_result = outcome
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    callback_result
}

fn log_window_facts(hwnd: HWND, owner_thread: u32, current_thread: u32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GWL_EXSTYLE, GWL_STYLE, WS_CHILD,
    };

    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let exstyle = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let parent = unsafe { GetParent(hwnd) }.unwrap_or_default();
    let owner = unsafe { GetWindow(hwnd, GW_OWNER) }.unwrap_or_default();
    let top_level = parent.0.is_null() && style & WS_CHILD.0 == 0;
    let dwm_host_backdrop = unsafe { material_backend::host_backdrop_enabled(hwnd) };
    eprintln!(
        "[native-material] hwnd={:#x} style={style:#010x} exstyle={exstyle:#010x} top_level={top_level} parent={:#x} owner={:#x} dwm_host_backdrop={dwm_host_backdrop} hwnd_thread={owner_thread} current_thread={current_thread}",
        hwnd.0 as usize,
        parent.0 as usize,
        owner.0 as usize
    );
}
