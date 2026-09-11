#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("native-composition-webview-poc only runs on Windows");
}

#[cfg(target_os = "windows")]
mod winappsdk;

#[cfg(target_os = "windows")]
mod windows_poc {
    use crate::winappsdk::Microsoft::UI::{
        Composition::SystemBackdrops::{
            DesktopAcrylicController, SystemBackdropConfiguration, SystemBackdropState,
            SystemBackdropTheme,
        },
        WindowId,
    };
    use std::{
        cell::RefCell,
        ffi::c_void,
        mem::size_of,
        sync::mpsc,
        time::{Duration, Instant},
    };
    use webview2_com::{
        CoreWebView2EnvironmentOptions, CreateCoreWebView2CompositionControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler,
        Microsoft::Web::WebView2::Win32::{
            CreateCoreWebView2Environment, CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2,
            ICoreWebView2CompositionController, ICoreWebView2Controller, ICoreWebView2Controller2,
            ICoreWebView2Controller3, ICoreWebView2Environment, ICoreWebView2Environment3,
            ICoreWebView2EnvironmentOptions, ICoreWebView2ProcessFailedEventArgs2,
            COREWEBVIEW2_BOUNDS_MODE, COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS, COREWEBVIEW2_COLOR,
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP, COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
            COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC, COREWEBVIEW2_PROCESS_FAILED_KIND,
            COREWEBVIEW2_PROCESS_FAILED_REASON, COREWEBVIEW2_WEB_ERROR_STATUS,
        },
        NavigationCompletedEventHandler, ProcessFailedEventHandler, WebMessageReceivedEventHandler,
    };
    use windows::{
        core::{IUnknown, Interface, Result as WinResult, BOOL, HRESULT, PCWSTR, PWSTR},
        System::DispatcherQueueController,
        Win32::{
            Foundation::{E_FAIL, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
            Graphics::{
                Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH},
                Gdi::{BeginPaint, EndPaint, HBRUSH, PAINTSTRUCT},
            },
            System::{
                Com::CoTaskMemFree,
                LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
                Threading::GetCurrentThreadId,
                WinRT::{
                    Composition::ICompositorDesktopInterop, CreateDispatcherQueueController,
                    DispatcherQueueOptions, RoInitialize, DQTAT_COM_ASTA, DQTYPE_THREAD_CURRENT,
                    RO_INIT_SINGLETHREADED,
                },
            },
            UI::{
                HiDpi::{
                    GetDpiForWindow, SetProcessDpiAwarenessContext,
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                },
                Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
                WindowsAndMessaging::{
                    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                    GetClientRect, GetMessageW, LoadCursorW, PostMessageW, PostQuitMessage,
                    RegisterClassW, SetForegroundWindow, ShowWindow, TranslateMessage, CS_HREDRAW,
                    CS_VREDRAW, IDC_ARROW, MSG, SW_SHOW, WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE,
                    WM_ACTIVATEAPP, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN,
                    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SIZE, WNDCLASSW,
                    WS_OVERLAPPEDWINDOW, WS_VISIBLE,
                },
            },
        },
        UI::Composition::{
            CompositionTarget, Compositor, ContainerVisual, Desktop::DesktopWindowTarget,
        },
    };
    use windows_numerics::{Vector2, Vector3};

    const WINDOW_CLASS: PCWSTR = windows::core::w!("CompositionWebViewPocWindow");
    const WINDOW_TITLE: PCWSTR = windows::core::w!("Phase 7C.0 - Acrylic + WebView2 Composition");

    type SelfContainedInitialize = unsafe extern "system" fn() -> HRESULT;

    struct RuntimeLifetime {
        _module: HMODULE,
    }

    thread_local! {
        static INPUT: RefCell<Option<InputBridge>> = const { RefCell::new(None) };
        static BACKDROP_CONFIGURATION: RefCell<Option<SystemBackdropConfiguration>> = const { RefCell::new(None) };
    }

    #[derive(Clone)]
    struct InputBridge {
        composition: ICoreWebView2CompositionController,
        controller: ICoreWebView2Controller,
        controller3: ICoreWebView2Controller3,
        root: ContainerVisual,
        webview_host: ContainerVisual,
    }

    struct PocState {
        hwnd: HWND,
        dispatcher: Option<DispatcherQueueController>,
        compositor: Option<Compositor>,
        desktop_target: Option<DesktopWindowTarget>,
        composition_target: Option<CompositionTarget>,
        root: Option<ContainerVisual>,
        webview_host: Option<ContainerVisual>,
        backdrop_configuration: Option<SystemBackdropConfiguration>,
        acrylic: Option<DesktopAcrylicController>,
        acrylic_state_changed_token: Option<i64>,
        acrylic_attached: bool,
        environment: Option<ICoreWebView2Environment>,
        composition_controller: Option<ICoreWebView2CompositionController>,
        controller: Option<ICoreWebView2Controller>,
        webview: Option<ICoreWebView2>,
        web_message_token: Option<i64>,
        navigation_completed_token: Option<i64>,
        process_failed_token: Option<i64>,
        shutdown_complete: bool,
    }

    impl PocState {
        fn new(hwnd: HWND) -> Self {
            Self {
                hwnd,
                dispatcher: None,
                compositor: None,
                desktop_target: None,
                composition_target: None,
                root: None,
                webview_host: None,
                backdrop_configuration: None,
                acrylic: None,
                acrylic_state_changed_token: None,
                acrylic_attached: false,
                environment: None,
                composition_controller: None,
                controller: None,
                webview: None,
                web_message_token: None,
                navigation_completed_token: None,
                process_failed_token: None,
                shutdown_complete: false,
            }
        }

        fn shutdown(&mut self) {
            if self.shutdown_complete {
                return;
            }
            eprintln!("[poc] lifecycle=detach-begin");
            INPUT.with(|slot| slot.borrow_mut().take());
            BACKDROP_CONFIGURATION.with(|slot| slot.borrow_mut().take());

            if let (Some(webview), Some(token)) = (&self.webview, self.web_message_token.take()) {
                unsafe {
                    if let Err(error) = webview.remove_WebMessageReceived(token) {
                        log_error("web_message_remove", &error);
                    }
                }
            }
            if let (Some(webview), Some(token)) =
                (&self.webview, self.navigation_completed_token.take())
            {
                unsafe {
                    let _ = webview.remove_NavigationCompleted(token);
                }
            }
            if let (Some(webview), Some(token)) = (&self.webview, self.process_failed_token.take())
            {
                unsafe {
                    let _ = webview.remove_ProcessFailed(token);
                }
            }
            if let Some(composition) = &self.composition_controller {
                unsafe {
                    match composition.SetRootVisualTarget(None::<&IUnknown>) {
                        Ok(()) => eprintln!("[poc] root_visual_target_set=false"),
                        Err(error) => log_error("root_visual_target_detach", &error),
                    }
                }
            }
            if let Some(controller) = &self.controller {
                unsafe {
                    match controller.Close() {
                        Ok(()) => eprintln!("[poc] webview_controller_closed=true"),
                        Err(error) => log_error("webview_controller_close", &error),
                    }
                }
            }
            self.webview.take();
            self.controller.take();
            self.composition_controller.take();
            self.environment.take();

            if let (Some(acrylic), Some(token)) =
                (&self.acrylic, self.acrylic_state_changed_token.take())
            {
                if let Err(error) = acrylic.RemoveStateChanged(token) {
                    log_error("acrylic_state_changed_remove", &error);
                }
            }
            if self.acrylic_attached {
                if let Some(acrylic) = &self.acrylic {
                    match acrylic.RemoveAllSystemBackdropTargets() {
                        Ok(()) => eprintln!("[poc] acrylic_detached=true"),
                        Err(error) => log_error("acrylic_detach", &error),
                    }
                }
                self.acrylic_attached = false;
            }
            if let Some(acrylic) = &self.acrylic {
                if let Err(error) = acrylic.Close() {
                    log_error("acrylic_close", &error);
                }
            }
            self.acrylic.take();
            self.backdrop_configuration.take();
            self.webview_host.take();
            self.root.take();
            self.composition_target.take();
            self.desktop_target.take();
            self.compositor.take();
            self.dispatcher.take();
            self.shutdown_complete = true;
            eprintln!("[poc] lifecycle=shutdown-complete");
        }
    }

    impl Drop for PocState {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn log_error(label: &str, error: &windows::core::Error) {
        eprintln!(
            "[poc] {label}=failed HRESULT={:#010x} error={error}",
            error.code().0 as u32
        );
    }

    fn callback_error(label: &str, error: impl std::fmt::Display) -> windows::core::Error {
        windows::core::Error::new(E_FAIL, format!("{label}: {error}"))
    }

    fn backdrop_state_name(value: SystemBackdropState) -> &'static str {
        if value == SystemBackdropState::Active {
            "active"
        } else if value == SystemBackdropState::Fallback {
            "fallback"
        } else if value == SystemBackdropState::HighContrast {
            "high-contrast"
        } else {
            "unknown"
        }
    }

    unsafe fn initialize_self_contained() -> WinResult<RuntimeLifetime> {
        let module = LoadLibraryW(windows::core::w!("Microsoft.WindowsAppRuntime.dll"))?;
        let address = GetProcAddress(
            module,
            windows::core::s!("WindowsAppRuntime_EnsureIsLoaded"),
        )
        .ok_or_else(windows::core::Error::from_win32)?;
        let initialize: SelfContainedInitialize = std::mem::transmute(address);
        let result = initialize();
        eprintln!(
            "[poc] windows_app_runtime_self_contained_hresult={:#010x}",
            result.0 as u32
        );
        result.ok()?;
        eprintln!("[poc] windows_app_runtime_self_contained=success");
        Ok(RuntimeLifetime { _module: module })
    }

    unsafe fn create_dispatcher() -> WinResult<DispatcherQueueController> {
        let dispatcher = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_ASTA,
        })?;
        eprintln!("[poc] dispatcher_queue=created current_thread ASTA");
        Ok(dispatcher)
    }

    unsafe fn create_window() -> WinResult<HWND> {
        let module = GetModuleHandleW(None)?;
        let instance = HINSTANCE(module.0);
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH::default(),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WINDOW_CLASS,
            WINDOW_TITLE,
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            180,
            140,
            900,
            620,
            None,
            None,
            Some(instance),
            None,
        )
    }

    unsafe fn window_client_rect(hwnd: HWND) -> WinResult<RECT> {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect)?;
        Ok(rect)
    }

    unsafe fn window_size(hwnd: HWND) -> WinResult<(i32, i32)> {
        let rect = window_client_rect(hwnd)?;
        Ok((rect.right - rect.left, rect.bottom - rect.top))
    }

    unsafe fn apply_host_geometry(hwnd: HWND, bridge: &InputBridge, reason: &str) -> WinResult<()> {
        // This PoC is Per-Monitor-V2 aware and explicitly uses WebView2 raw-pixel bounds.
        // Consequently these three layers intentionally receive the same physical-pixel size;
        // no DPI multiplication or division belongs in this path.
        let client = window_client_rect(hwnd)?;
        let width = client.right - client.left;
        let height = client.bottom - client.top;
        if width <= 0 || height <= 0 {
            eprintln!("[poc] geometry reason={reason} skipped=empty-client");
            return Ok(());
        }
        let size = Vector2 {
            X: width as f32,
            Y: height as f32,
        };
        let dpi = GetDpiForWindow(hwnd);
        let dpi_scale = dpi as f64 / 96.0;
        let zero = Vector3::default();
        let identity = Vector3 {
            X: 1.0,
            Y: 1.0,
            Z: 1.0,
        };
        bridge.root.SetOffset(zero)?;
        bridge.root.SetScale(identity)?;
        bridge.root.SetSize(size)?;
        bridge.webview_host.SetOffset(zero)?;
        bridge.webview_host.SetScale(identity)?;
        bridge.webview_host.SetSize(size)?;
        bridge
            .controller3
            .SetShouldDetectMonitorScaleChanges(false)?;
        bridge.controller3.SetRasterizationScale(dpi_scale)?;
        bridge.controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        })?;

        let root_size = bridge.root.Size()?;
        let root_offset = bridge.root.Offset()?;
        let root_scale = bridge.root.Scale()?;
        let host_size = bridge.webview_host.Size()?;
        let host_offset = bridge.webview_host.Offset()?;
        let host_scale = bridge.webview_host.Scale()?;
        let mut bounds = RECT::default();
        bridge.controller.Bounds(&mut bounds)?;
        let mut zoom = 0.0;
        bridge.controller.ZoomFactor(&mut zoom)?;
        let mut rasterization_scale = 0.0;
        bridge
            .controller3
            .RasterizationScale(&mut rasterization_scale)?;
        let mut bounds_mode = COREWEBVIEW2_BOUNDS_MODE::default();
        bridge.controller3.BoundsMode(&mut bounds_mode)?;
        let mut auto_scale = BOOL(0);
        bridge
            .controller3
            .ShouldDetectMonitorScaleChanges(&mut auto_scale)?;

        eprintln!(
            "[poc] geometry reason={reason} hwnd_client_rect_physical_px=({},{})-({},{}) size={}x{}",
            client.left, client.top, client.right, client.bottom, width, height
        );
        eprintln!(
            "[poc] geometry dpi={} scale_factor={:.4} units=physical-px no-dpi-conversion",
            dpi, dpi_scale
        );
        eprintln!(
            "[poc] geometry root_container_size=({:.1},{:.1}) offset=({:.1},{:.1},{:.1}) scale=({:.1},{:.1},{:.1})",
            root_size.X,
            root_size.Y,
            root_offset.X,
            root_offset.Y,
            root_offset.Z,
            root_scale.X,
            root_scale.Y,
            root_scale.Z
        );
        eprintln!(
            "[poc] geometry webview_container_size=({:.1},{:.1}) offset=({:.1},{:.1},{:.1}) scale=({:.1},{:.1},{:.1})",
            host_size.X,
            host_size.Y,
            host_offset.X,
            host_offset.Y,
            host_offset.Z,
            host_scale.X,
            host_scale.Y,
            host_scale.Z
        );
        eprintln!(
            "[poc] geometry composition_controller_bounds=({},{})-({},{}) size={}x{} bounds_mode={} rasterization_scale={:.4} zoom_factor={:.4} auto_monitor_scale={}",
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            bounds_mode.0,
            rasterization_scale,
            zoom,
            auto_scale.as_bool()
        );
        Ok(())
    }

    fn point_from_lparam(lparam: LPARAM) -> POINT {
        POINT {
            x: (lparam.0 as u32 & 0xffff) as u16 as i16 as i32,
            y: ((lparam.0 as u32 >> 16) & 0xffff) as u16 as i16 as i32,
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut paint);
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_SIZE => {
                INPUT.with(|slot| {
                    if let Some(bridge) = slot.borrow().as_ref() {
                        if let Err(error) = apply_host_geometry(hwnd, bridge, "WM_SIZE") {
                            log_error("geometry_resize", &error);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_DPICHANGED => {
                INPUT.with(|slot| {
                    if let Some(bridge) = slot.borrow().as_ref() {
                        if let Err(error) = apply_host_geometry(hwnd, bridge, "WM_DPICHANGED") {
                            log_error("geometry_dpi_changed", &error);
                        }
                    }
                });
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_ACTIVATE => {
                let active = (wparam.0 & 0xffff) != WA_INACTIVE as usize;
                BACKDROP_CONFIGURATION.with(|slot| {
                    if let Some(configuration) = slot.borrow().as_ref() {
                        if let Err(error) = configuration.SetIsInputActive(active) {
                            log_error("acrylic_input_active", &error);
                        }
                    }
                });
                eprintln!("[poc] window_activation={active}");
                LRESULT(0)
            }
            WM_ACTIVATEAPP => {
                let active = wparam.0 != 0;
                BACKDROP_CONFIGURATION.with(|slot| {
                    if let Some(configuration) = slot.borrow().as_ref() {
                        if let Err(error) = configuration.SetIsInputActive(active) {
                            log_error("acrylic_input_active", &error);
                        }
                    }
                });
                eprintln!("[poc] app_activation={active}");
                LRESULT(0)
            }
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
                INPUT.with(|slot| {
                    if let Some(bridge) = slot.borrow().as_ref() {
                        let kind = match message {
                            WM_LBUTTONDOWN => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
                            WM_LBUTTONUP => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
                            _ => COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                        };
                        if message == WM_LBUTTONDOWN {
                            let _ = SetCapture(hwnd);
                            let _ = bridge
                                .controller
                                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                        }
                        let result = bridge.composition.SendMouseInput(
                            kind,
                            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                            0,
                            point_from_lparam(lparam),
                        );
                        if let Err(error) = result {
                            log_error("mouse_forward", &error);
                        }
                        if message == WM_LBUTTONUP {
                            let _ = ReleaseCapture();
                        }
                    }
                });
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 == 0x1b => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_CLOSE => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    unsafe fn create_composition(state: &mut PocState) -> WinResult<()> {
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(state.hwnd, false)?;
        let root = compositor.CreateContainerVisual()?;
        let webview_host = compositor.CreateContainerVisual()?;
        root.Children()?.InsertAtTop(&webview_host)?;
        target.SetRoot(&root)?;
        let composition_target: CompositionTarget = target.cast()?;
        let (width, height) = window_size(state.hwnd)?;
        let size = Vector2 {
            X: width as f32,
            Y: height as f32,
        };
        root.SetSize(size)?;
        webview_host.SetSize(size)?;
        root.SetOffset(Vector3::default())?;
        webview_host.SetOffset(Vector3::default())?;

        eprintln!("[poc] desktop_window_target=created is_topmost=false");
        eprintln!("[poc] root_visual=ContainerVisual");
        eprintln!("[poc] webview_host_visual=ContainerVisual child-of-root");
        eprintln!(
            "[poc] composition_identity=target:{:p} root:{:p} webview_host:{:p}",
            Interface::as_raw(&target),
            Interface::as_raw(&root),
            Interface::as_raw(&webview_host)
        );

        state.compositor = Some(compositor);
        state.desktop_target = Some(target);
        state.composition_target = Some(composition_target);
        state.root = Some(root);
        state.webview_host = Some(webview_host);
        Ok(())
    }

    unsafe fn attach_acrylic(state: &mut PocState) -> WinResult<()> {
        let enabled = BOOL(1);
        DwmSetWindowAttribute(
            state.hwnd,
            DWMWA_USE_HOSTBACKDROPBRUSH,
            &enabled as *const _ as *const c_void,
            size_of::<BOOL>() as u32,
        )?;
        let configuration = SystemBackdropConfiguration::new()?;
        configuration.SetIsInputActive(true)?;
        configuration.SetTheme(SystemBackdropTheme::Dark)?;
        let controller = DesktopAcrylicController::new()?;
        let window_id = WindowId {
            Value: state.hwnd.0 as usize as u64,
        };
        let target = state
            .composition_target
            .as_ref()
            .ok_or_else(|| windows::core::Error::new(E_FAIL, "missing CompositionTarget"))?;
        let attached = controller.SetTargetWithWindowId(window_id, target)?;
        eprintln!("[poc] acrylic_attach_result={attached}");
        if !attached {
            return Err(windows::core::Error::new(E_FAIL, "Acrylic target rejected"));
        }
        let acrylic_state = controller.State()?;
        eprintln!("[poc] acrylic_state={}", backdrop_state_name(acrylic_state));
        let callback_controller = controller.clone();
        let state_changed = windows::Foundation::TypedEventHandler::new(move |_, _| {
            let current = callback_controller.State()?;
            eprintln!("[poc] acrylic_state={}", backdrop_state_name(current));
            Ok(())
        });
        let state_changed_token = controller.StateChanged(&state_changed)?;
        BACKDROP_CONFIGURATION.with(|slot| {
            *slot.borrow_mut() = Some(configuration.clone());
        });
        state.backdrop_configuration = Some(configuration);
        state.acrylic = Some(controller);
        state.acrylic_state_changed_token = Some(state_changed_token);
        state.acrylic_attached = true;
        Ok(())
    }

    unsafe fn create_webview_environment(
        software_rendering: bool,
    ) -> WinResult<ICoreWebView2Environment> {
        let (sender, receiver) = mpsc::channel();
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |result, environment| {
                let _ = sender.send((result, environment));
                Ok(())
            },
        ));
        if software_rendering {
            let options = CoreWebView2EnvironmentOptions::default();
            options.set_additional_browser_arguments("--disable-gpu".into());
            let options: ICoreWebView2EnvironmentOptions = options.into();
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR::null(),
                &options,
                &handler,
            )?;
            eprintln!("[poc] webview_rendering_mode=software-diagnostic");
        } else {
            CreateCoreWebView2Environment(&handler)?;
            eprintln!("[poc] webview_rendering_mode=default-hardware");
        }
        let (result, environment) = webview2_com::wait_with_pump(receiver)
            .map_err(|error| callback_error("environment callback", error))?;
        result?;
        environment.ok_or_else(|| windows::core::Error::new(E_FAIL, "environment was null"))
    }

    unsafe fn create_composition_controller(
        environment: &ICoreWebView2Environment,
        hwnd: HWND,
    ) -> WinResult<ICoreWebView2CompositionController> {
        let environment3: ICoreWebView2Environment3 = environment.cast()?;
        let (sender, receiver) = mpsc::channel();
        let handler = CreateCoreWebView2CompositionControllerCompletedHandler::create(Box::new(
            move |result, controller| {
                let result_code = match &result {
                    Ok(()) => 0_u32,
                    Err(error) => error.code().0 as u32,
                };
                eprintln!(
                    "[poc] composition_controller_creation_callback_hresult={result_code:#010x}"
                );
                let _ = sender.send((result, controller));
                Ok(())
            },
        ));
        let call_result = environment3.CreateCoreWebView2CompositionController(hwnd, &handler);
        match &call_result {
            Ok(()) => eprintln!("[poc] composition_controller_creation_call_hresult=0x00000000"),
            Err(error) => eprintln!(
                "[poc] composition_controller_creation_call_hresult={:#010x}",
                error.code().0 as u32
            ),
        }
        call_result?;
        let (result, controller) = webview2_com::wait_with_pump(receiver)
            .map_err(|error| callback_error("composition controller callback", error))?;
        result?;
        controller
            .ok_or_else(|| windows::core::Error::new(E_FAIL, "composition controller was null"))
    }

    unsafe fn take_pwstr(value: PWSTR) -> String {
        if value.is_null() {
            return "<null>".into();
        }
        let result = value.to_string().unwrap_or_else(|_| "<invalid>".into());
        CoTaskMemFree(Some(value.0.cast()));
        result
    }

    unsafe fn attach_webview(state: &mut PocState, software_rendering: bool) -> WinResult<()> {
        let environment = create_webview_environment(software_rendering)?;
        let mut version = PWSTR::null();
        environment.BrowserVersionString(&mut version)?;
        eprintln!("[poc] webview_runtime_version={}", take_pwstr(version));

        let composition = create_composition_controller(&environment, state.hwnd)?;
        let controller: ICoreWebView2Controller = composition.cast()?;
        let controller3: ICoreWebView2Controller3 = controller.cast()?;
        controller3.SetBoundsMode(COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS)?;
        eprintln!("[poc] webview_controller_type=ICoreWebView2CompositionController");
        eprintln!("[poc] webview_composition_controller_created=true");
        eprintln!("[poc] webview_bounds_mode=raw-physical-pixels");

        let transparent: ICoreWebView2Controller2 = controller.cast()?;
        transparent.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
            A: 0,
            R: 0,
            G: 0,
            B: 0,
        })?;
        let (width, height) = window_size(state.hwnd)?;
        controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        })?;
        controller.SetIsVisible(true)?;

        let webview = controller.CoreWebView2()?;
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
            if let Some(args) = args {
                let mut message = PWSTR::null();
                unsafe { args.TryGetWebMessageAsString(&mut message)? };
                let message = unsafe { take_pwstr(message) };
                match message.as_str() {
                    "content-ready" => eprintln!("[poc] webview_content_ready=true"),
                    "pointer" => {
                        eprintln!("[poc] pointer_event_received=true source=web-message")
                    }
                    value if value.starts_with("page-state:") => {
                        eprintln!("[poc] web_content_{}", &value["page-state:".len()..])
                    }
                    _ => eprintln!("[poc] web_message={message}"),
                }
            }
            Ok(())
        }));
        let mut token = 0_i64;
        webview.add_WebMessageReceived(&handler, &mut token)?;
        let navigation_handler =
            NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                if let Some(args) = args {
                    let mut success = BOOL(0);
                    unsafe { args.IsSuccess(&mut success)? };
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                    unsafe { args.WebErrorStatus(&mut status)? };
                    eprintln!(
                        "[poc] navigation_completed=true success={} web_error_status={}",
                        success.as_bool(),
                        status.0
                    );
                }
                if let Some(sender) = sender {
                    let mut source = PWSTR::null();
                    unsafe { sender.Source(&mut source)? };
                    eprintln!("[poc] web_content_actual_url={}", unsafe {
                        take_pwstr(source)
                    });
                }
                Ok(())
            }));
        let mut navigation_token = 0_i64;
        webview.add_NavigationCompleted(&navigation_handler, &mut navigation_token)?;
        let process_failed_handler = ProcessFailedEventHandler::create(Box::new(
            move |_sender, args| {
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                if let Some(args) = args {
                    unsafe { args.ProcessFailedKind(&mut kind)? };
                    if let Ok(args2) = args.cast::<ICoreWebView2ProcessFailedEventArgs2>() {
                        let mut reason = COREWEBVIEW2_PROCESS_FAILED_REASON::default();
                        let mut exit_code = 0_i32;
                        unsafe {
                            args2.Reason(&mut reason)?;
                            args2.ExitCode(&mut exit_code)?;
                        }
                        eprintln!(
                            "[poc] webview_process_failure_detail=kind:{} reason:{} exit_code:{:#010x}",
                            kind.0, reason.0, exit_code as u32
                        );
                    }
                }
                eprintln!("[poc] webview_process_failed=true kind={}", kind.0);
                Ok(())
            },
        ));
        let mut process_failed_token = 0_i64;
        webview.add_ProcessFailed(&process_failed_handler, &mut process_failed_token)?;
        let html = windows::core::w!(
            r#"<!doctype html>
<html class="composition-poc-page" data-entrypoint="phase-7c0-inline" data-state="expanded"><head><meta charset="utf-8"><title>Phase 7C.0 Web Content Probe</title><style>
html,body{width:100%;height:100%;margin:0;background:transparent;color:#fff;font-family:Segoe UI,sans-serif}
body{display:grid;place-items:center;overflow:hidden}
button{font:600 28px Segoe UI;padding:28px 38px;color:white;background:rgba(18,22,30,.58);border:1px solid rgba(255,255,255,.42);border-radius:18px;box-shadow:0 18px 60px rgba(0,0,0,.25)}
</style></head><body><button id="hello">Hello WebView2 Composition</button>
<script>
hello.addEventListener('pointerdown',()=>{hello.textContent='Pointer received';chrome.webview.postMessage('pointer')});
requestAnimationFrame(()=>requestAnimationFrame(()=>{
  const root=document.documentElement;
  const orb=document.querySelector('.floating-orb,[data-state="collapsed"],[data-mode="orb"]');
  const orbRect=orb?.getBoundingClientRect();
  chrome.webview.postMessage('page-state:'+JSON.stringify({
    entrypoint:root.dataset.entrypoint||null,
    url:location.href,
    title:document.title,
    rootClass:root.className,
    rootDataState:root.dataset.state||null,
    bodyClass:document.body.className,
    collapsed:root.dataset.state==='collapsed'||root.classList.contains('collapsed'),
    orbPresent:Boolean(orb),
    orbRect:orbRect?{x:orbRect.x,y:orbRect.y,width:orbRect.width,height:orbRect.height}:null,
    innerWidth:window.innerWidth,
    innerHeight:window.innerHeight,
    devicePixelRatio:window.devicePixelRatio
  }));
  chrome.webview.postMessage('content-ready');
}));
</script>
</body></html>"#
        );
        eprintln!("[poc] web_content_navigation_api=NavigateToString");
        eprintln!("[poc] web_content_source=inline-html entrypoint=phase-7c0-inline");
        eprintln!("[poc] web_content_window_mode=standalone-poc source=hard-coded");
        eprintln!("[poc] web_content_expected_state=expanded orb_css_present=false");
        webview.NavigateToString(html)?;
        eprintln!("[poc] webview_navigation=minimal-inline-html");

        let host = state
            .webview_host
            .as_ref()
            .ok_or_else(|| windows::core::Error::new(E_FAIL, "missing WebView host visual"))?;
        composition.SetRootVisualTarget(host)?;
        let _commit = state
            .compositor
            .as_ref()
            .ok_or_else(|| windows::core::Error::new(E_FAIL, "missing compositor"))?
            .RequestCommitAsync()?;
        eprintln!("[poc] composition_commit_requested=true");
        let round_trip = composition.RootVisualTarget()?;
        eprintln!("[poc] root_visual_target_set=true");
        eprintln!(
            "[poc] root_visual_target_identity=expected:{:p} actual:{:p}",
            Interface::as_raw(host),
            Interface::as_raw(&round_trip)
        );

        let bridge = InputBridge {
            composition: composition.clone(),
            controller: controller.clone(),
            controller3,
            root: state.root.as_ref().unwrap().clone(),
            webview_host: host.clone(),
        };
        apply_host_geometry(state.hwnd, &bridge, "initial-layout")?;
        INPUT.with(|slot| *slot.borrow_mut() = Some(bridge));

        state.environment = Some(environment);
        state.composition_controller = Some(composition);
        state.controller = Some(controller);
        state.webview = Some(webview);
        state.web_message_token = Some(token);
        state.navigation_completed_token = Some(navigation_token);
        state.process_failed_token = Some(process_failed_token);
        eprintln!("[poc] lifecycle=attach-complete");
        Ok(())
    }

    unsafe fn run() -> WinResult<()> {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        eprintln!("[poc] dpi_awareness=per-monitor-v2");
        let mut qa_seconds = None;
        let mut without_acrylic = false;
        let mut software_rendering = false;
        let mut system_composition_only = false;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--qa-seconds" => {
                    qa_seconds = args.next().and_then(|value| value.parse::<u64>().ok());
                }
                "--without-acrylic" => without_acrylic = true,
                "--software-rendering" => software_rendering = true,
                "--system-composition-only" => {
                    system_composition_only = true;
                    without_acrylic = true;
                }
                _ => {}
            }
        }

        let _runtime = if system_composition_only {
            eprintln!("[poc] windows_app_runtime=skipped-negative-control");
            None
        } else {
            Some(initialize_self_contained()?)
        };
        RoInitialize(RO_INIT_SINGLETHREADED)?;
        eprintln!("[poc] apartment=single_threaded");
        let dispatcher = create_dispatcher()?;
        let hwnd = create_window()?;
        let (width, height) = window_size(hwnd)?;
        eprintln!("[poc] hwnd={:#x}", hwnd.0 as usize);
        eprintln!("[poc] thread_id={}", GetCurrentThreadId());
        eprintln!("[poc] dpi={}", GetDpiForWindow(hwnd));
        eprintln!("[poc] size={}x{}", width, height);
        eprintln!("[poc] lifecycle=attach-begin");

        let mut state = PocState::new(hwnd);
        state.dispatcher = Some(dispatcher);
        create_composition(&mut state)?;
        if without_acrylic {
            eprintln!("[poc] acrylic_probe=skipped-negative-control");
        } else {
            attach_acrylic(&mut state)?;
        }
        attach_webview(&mut state, software_rendering)?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let foreground_result = SetForegroundWindow(hwnd).as_bool();
        eprintln!("[poc] set_foreground_window={foreground_result}");

        let qa_started = Instant::now();
        let deadline = qa_seconds.map(|seconds| qa_started + Duration::from_secs(seconds));
        let mut pointer_probe_sent = false;
        let mut message = MSG::default();
        loop {
            if qa_seconds.is_some()
                && !pointer_probe_sent
                && Instant::now().duration_since(qa_started) >= Duration::from_secs(2)
            {
                let (width, height) = window_size(hwnd)?;
                let x = width / 2;
                let y = height / 2;
                let packed = ((y as u32) << 16) | (x as u32 & 0xffff);
                PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), LPARAM(packed as isize))?;
                PostMessageW(
                    Some(hwnd),
                    WM_LBUTTONDOWN,
                    WPARAM(0),
                    LPARAM(packed as isize),
                )?;
                PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), LPARAM(packed as isize))?;
                pointer_probe_sent = true;
                eprintln!("[poc] synthetic_pointer_probe=posted point={x},{y}");
            }
            if deadline.is_some_and(|value| Instant::now() >= value) {
                eprintln!("[poc] qa_timeout_reached=true");
                break;
            }
            let result = if deadline.is_some() {
                windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                    &mut message,
                    None,
                    0,
                    0,
                    windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
                )
                .as_bool()
            } else {
                GetMessageW(&mut message, None, 0, 0).0 > 0
            };
            if result {
                if message.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            } else if deadline.is_some() {
                std::thread::sleep(Duration::from_millis(10));
            } else {
                break;
            }
        }
        state.shutdown();
        let _ = DestroyWindow(hwnd);
        Ok(())
    }

    pub fn main() {
        if let Err(error) = unsafe { run() } {
            eprintln!(
                "[poc] fatal_hresult={:#010x} error={error}",
                error.code().0 as u32
            );
            std::process::exit(1);
        }
    }
}

#[cfg(target_os = "windows")]
fn main() {
    windows_poc::main();
}
