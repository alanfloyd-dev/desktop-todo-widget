#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("native-acrylic-poc only runs on Windows");
}

#[cfg(target_os = "windows")]
mod winappsdk;

#[cfg(target_os = "windows")]
mod windows_poc {
    use crate::winappsdk::Microsoft::UI::{
        Composition::SystemBackdrops::{
            DesktopAcrylicController, SystemBackdropConfiguration, SystemBackdropTheme,
        },
        WindowId,
    };
    use std::{
        ffi::c_void,
        fs::File,
        io::Write,
        mem::size_of,
        path::Path,
        time::{Duration, Instant},
    };
    use windows::{
        core::{Interface, Result as WinResult, BOOL, HRESULT},
        Graphics::{
            Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem},
            DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        },
        Win32::{
            Foundation::{
                COLORREF, ERROR_INSUFFICIENT_BUFFER, FARPROC, HINSTANCE, HMODULE, HWND, LPARAM,
                LRESULT, RECT, WPARAM,
            },
            Graphics::{
                Direct3D::D3D_DRIVER_TYPE_HARDWARE,
                Direct3D11::{
                    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
                    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
                },
                Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH},
                Dxgi::IDXGIDevice,
                Gdi::{
                    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
                    CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, GetDC, GetDIBits,
                    GetStockObject, LineTo, MoveToEx, ReleaseDC, SelectObject, SetBkMode,
                    SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
                    HBRUSH, HGDIOBJ, PAINTSTRUCT, SRCCOPY, TRANSPARENT, WHITE_BRUSH,
                },
            },
            Storage::{
                FileSystem::{
                    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
                },
                Packaging::Appx::{GetCurrentPackageInfo, PACKAGE_FILTER_DYNAMIC, PACKAGE_INFO},
            },
            System::{
                LibraryLoader::{
                    GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LoadLibraryW,
                },
                WinRT::{
                    Composition::ICompositorDesktopInterop,
                    CreateDispatcherQueueController,
                    Direct3D11::{
                        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
                    },
                    DispatcherQueueOptions,
                    Graphics::Capture::IGraphicsCaptureItemInterop,
                    RoInitialize, DQTAT_COM_ASTA, DQTYPE_THREAD_CURRENT, RO_INIT_SINGLETHREADED,
                },
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClassLongPtrW, GetMessageW,
                GetWindowLongPtrW, LoadCursorW, PeekMessageW, PostQuitMessage, RegisterClassW,
                SetWindowTextW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
                GCLP_HBRBACKGROUND, GWL_EXSTYLE, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW,
                WINDOW_EX_STYLE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_PAINT, WM_QUIT,
                WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
            },
        },
        UI::Composition::{Compositor, ContainerVisual, Desktop::DesktopWindowTarget},
    };

    const FIXTURE_CLASS: windows::core::PCWSTR = windows::core::w!("AlanAcrylicFixture");
    const ACRYLIC_CLASS: windows::core::PCWSTR = windows::core::w!("AlanAcrylicTarget");
    const WINDOWS_APP_SDK_MAJOR_MINOR: u32 = 0x0001_0008;
    const WINDOWS_APP_RUNTIME_MIN_VERSION: u64 = 0x1f40_03b2_06a5_0000;

    type BootstrapInitialize =
        unsafe extern "system" fn(u32, windows::core::PCWSTR, u64) -> HRESULT;
    type BootstrapShutdown = unsafe extern "system" fn();
    type SelfContainedInitialize = unsafe extern "system" fn() -> HRESULT;

    struct BootstrapLifetime {
        _module: HMODULE,
        shutdown: BootstrapShutdown,
    }

    struct SelfContainedLifetime {
        _module: HMODULE,
    }

    impl Drop for BootstrapLifetime {
        fn drop(&mut self) {
            unsafe { (self.shutdown)() };
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum QaVariant {
        Only,
        Dispatcher,
        Compositor,
        Target,
        Controller,
        ControllerConfig,
        ControllerTarget,
        Full,
        FullNoRoot,
        TargetThenConfig,
    }

    impl QaVariant {
        fn parse(value: &str) -> Option<Self> {
            match value {
                "runtime-only" => Some(Self::Only),
                "runtime-dispatcher" => Some(Self::Dispatcher),
                "runtime-compositor" => Some(Self::Compositor),
                "runtime-target" => Some(Self::Target),
                "runtime-controller" => Some(Self::Controller),
                "runtime-controller-config" => Some(Self::ControllerConfig),
                "runtime-controller-target" => Some(Self::ControllerTarget),
                "runtime-full" => Some(Self::Full),
                "runtime-full-no-root" => Some(Self::FullNoRoot),
                "runtime-target-then-config" => Some(Self::TargetThenConfig),
                _ => None,
            }
        }

        fn needs_dispatcher(self) -> bool {
            self != Self::Only
        }

        fn needs_compositor(self) -> bool {
            matches!(
                self,
                Self::Compositor
                    | Self::Target
                    | Self::ControllerTarget
                    | Self::Full
                    | Self::FullNoRoot
                    | Self::TargetThenConfig
            )
        }

        fn needs_target(self) -> bool {
            matches!(
                self,
                Self::Target
                    | Self::ControllerTarget
                    | Self::Full
                    | Self::FullNoRoot
                    | Self::TargetThenConfig
            )
        }

        fn needs_controller(self) -> bool {
            matches!(
                self,
                Self::Controller
                    | Self::ControllerConfig
                    | Self::ControllerTarget
                    | Self::Full
                    | Self::FullNoRoot
                    | Self::TargetThenConfig
            )
        }

        fn needs_configuration(self) -> bool {
            matches!(
                self,
                Self::ControllerConfig | Self::Full | Self::FullNoRoot | Self::TargetThenConfig
            )
        }

        fn needs_set_target(self) -> bool {
            matches!(
                self,
                Self::ControllerTarget | Self::Full | Self::FullNoRoot | Self::TargetThenConfig
            )
        }
    }

    struct PocState {
        dispatcher: Option<windows::System::DispatcherQueueController>,
        compositor: Option<Compositor>,
        target: Option<DesktopWindowTarget>,
        composition_target: Option<windows::UI::Composition::CompositionTarget>,
        root: Option<ContainerVisual>,
        window_id: Option<WindowId>,
        configuration: Option<SystemBackdropConfiguration>,
        controller: Option<DesktopAcrylicController>,
        target_attached: bool,
        shutdown_complete: bool,
    }

    impl PocState {
        fn new() -> Self {
            Self {
                dispatcher: None,
                compositor: None,
                target: None,
                composition_target: None,
                root: None,
                window_id: None,
                configuration: None,
                controller: None,
                target_attached: false,
                shutdown_complete: false,
            }
        }

        fn shutdown(&mut self) {
            if self.shutdown_complete {
                return;
            }
            if self.target_attached {
                if let Some(controller) = self.controller.as_ref() {
                    match controller.RemoveAllSystemBackdropTargets() {
                        Ok(()) => eprintln!("[poc] shutdown=targets-removed"),
                        Err(error) => eprintln!(
                            "[poc] shutdown=remove-targets-failed HRESULT={:#010x}",
                            error.code().0 as u32
                        ),
                    }
                }
                self.target_attached = false;
            }
            if let Some(controller) = self.controller.as_ref() {
                match controller.Close() {
                    Ok(()) => eprintln!("[poc] shutdown=controller-closed"),
                    Err(error) => eprintln!(
                        "[poc] shutdown=controller-close-failed HRESULT={:#010x}",
                        error.code().0 as u32
                    ),
                }
            }
            self.controller.take();
            self.configuration.take();
            self.window_id.take();
            self.root.take();
            self.composition_target.take();
            self.target.take();
            self.compositor.take();
            self.dispatcher.take();
            self.shutdown_complete = true;
            eprintln!("[poc] shutdown=winrt-and-composition-released");
        }
    }

    impl Drop for PocState {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    unsafe fn required_export<T: Copy>(
        module: HMODULE,
        name: windows::core::PCSTR,
    ) -> WinResult<T> {
        let address: FARPROC = GetProcAddress(module, name);
        let Some(address) = address else {
            return Err(windows::core::Error::from_win32());
        };
        Ok(std::mem::transmute_copy(&address))
    }

    fn process_architecture() -> &'static str {
        #[cfg(target_arch = "x86_64")]
        return "x64";
        #[cfg(target_arch = "x86")]
        return "x86";
        #[cfg(target_arch = "aarch64")]
        return "arm64";
        #[allow(unreachable_code)]
        "unknown"
    }

    unsafe fn module_file_version(module: HMODULE) -> WinResult<String> {
        let mut path = vec![0_u16; 32_768];
        let path_length = GetModuleFileNameW(Some(module), &mut path) as usize;
        if path_length == 0 || path_length >= path.len() {
            return Err(windows::core::Error::from_win32());
        }
        path.truncate(path_length + 1);

        let info_size = GetFileVersionInfoSizeW(windows::core::PCWSTR(path.as_ptr()), None);
        if info_size == 0 {
            return Err(windows::core::Error::from_win32());
        }
        let mut info = vec![0_u8; info_size as usize];
        GetFileVersionInfoW(
            windows::core::PCWSTR(path.as_ptr()),
            None,
            info_size,
            info.as_mut_ptr().cast(),
        )?;

        let mut fixed = std::ptr::null_mut::<c_void>();
        let mut fixed_size = 0_u32;
        VerQueryValueW(
            info.as_ptr().cast(),
            windows::core::w!("\\"),
            &mut fixed,
            &mut fixed_size,
        )
        .ok()?;
        if fixed.is_null() || fixed_size < size_of::<VS_FIXEDFILEINFO>() as u32 {
            return Err(windows::core::Error::new(
                HRESULT(0x80004005_u32 as i32),
                "bootstrap DLL has no fixed file version",
            ));
        }
        let fixed = std::ptr::read_unaligned(fixed.cast::<VS_FIXEDFILEINFO>());
        Ok(format!(
            "{}.{}.{}.{}",
            fixed.dwFileVersionMS >> 16,
            fixed.dwFileVersionMS & 0xffff,
            fixed.dwFileVersionLS >> 16,
            fixed.dwFileVersionLS & 0xffff
        ))
    }

    unsafe fn log_resolved_runtime() -> WinResult<()> {
        let mut byte_length = 0_u32;
        let mut count = 0_u32;
        let probe = GetCurrentPackageInfo(
            PACKAGE_FILTER_DYNAMIC,
            &mut byte_length,
            None,
            Some(&mut count),
        );
        if probe != ERROR_INSUFFICIENT_BUFFER {
            return Err(HRESULT::from_win32(probe.0).into());
        }

        let word_count = (byte_length as usize).div_ceil(size_of::<usize>());
        let mut buffer = vec![0_usize; word_count];
        let result = GetCurrentPackageInfo(
            PACKAGE_FILTER_DYNAMIC,
            &mut byte_length,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut count),
        );
        if result.0 != 0 {
            return Err(HRESULT::from_win32(result.0).into());
        }

        let packages =
            std::slice::from_raw_parts(buffer.as_ptr().cast::<PACKAGE_INFO>(), count as usize);
        for package in packages {
            let full_name = std::ptr::addr_of!(package.packageFullName)
                .read_unaligned()
                .to_string()?;
            if full_name.starts_with("Microsoft.WindowsAppRuntime.1.8_") {
                eprintln!("[poc] resolved_runtime={full_name}");
                eprintln!("[poc] runtime_architecture={}", process_architecture());
                return Ok(());
            }
        }

        Err(windows::core::Error::new(
            HRESULT(0x80004005_u32 as i32),
            "bootstrap succeeded but no Windows App Runtime framework was found in the dynamic package graph",
        ))
    }

    unsafe fn initialize_bootstrap() -> WinResult<BootstrapLifetime> {
        eprintln!("[poc] requested_runtime_major_minor=0x{WINDOWS_APP_SDK_MAJOR_MINOR:08x}");
        eprintln!("[poc] requested_runtime_tag=<stable-empty>");
        eprintln!("[poc] requested_min_version=8000.946.1701.0");
        eprintln!("[poc] process_arch={}", process_architecture());
        let module = LoadLibraryW(windows::core::w!(
            "Microsoft.WindowsAppRuntime.Bootstrap.dll"
        ))?;
        eprintln!(
            "[poc] bootstrap_dll_version={}",
            module_file_version(module)?
        );
        let initialize: BootstrapInitialize =
            required_export(module, windows::core::s!("MddBootstrapInitialize"))?;
        let shutdown: BootstrapShutdown =
            required_export(module, windows::core::s!("MddBootstrapShutdown"))?;
        let bootstrap_result = initialize(
            WINDOWS_APP_SDK_MAJOR_MINOR,
            windows::core::w!(""),
            WINDOWS_APP_RUNTIME_MIN_VERSION,
        )
        .ok();
        if let Err(error) = bootstrap_result {
            eprintln!(
                "[poc] windows_app_runtime_bootstrap=failed HRESULT={:#010x}",
                error.code().0 as u32
            );
            return Err(error);
        }
        eprintln!("[poc] windows_app_runtime_bootstrap=success");
        log_resolved_runtime()?;
        Ok(BootstrapLifetime {
            _module: module,
            shutdown,
        })
    }

    unsafe fn initialize_self_contained() -> WinResult<SelfContainedLifetime> {
        eprintln!("[poc] deployment=self-contained");
        eprintln!("[poc] process_arch={}", process_architecture());
        let module = LoadLibraryW(windows::core::w!("Microsoft.WindowsAppRuntime.dll"))?;
        eprintln!(
            "[poc] windows_app_runtime_dll_version={}",
            module_file_version(module)?
        );
        let initialize: SelfContainedInitialize = required_export(
            module,
            windows::core::s!("WindowsAppRuntime_EnsureIsLoaded"),
        )?;
        let result = initialize().ok();
        if let Err(error) = result {
            eprintln!(
                "[poc] windows_app_runtime_self_contained=failed HRESULT={:#010x}",
                error.code().0 as u32
            );
            return Err(error);
        }
        eprintln!("[poc] windows_app_runtime_self_contained=success");
        eprintln!("[poc] resolved_runtime=self-contained-1.8.260804001");
        eprintln!("[poc] runtime_architecture={}", process_architecture());
        Ok(SelfContainedLifetime { _module: module })
    }

    fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
        COLORREF(red as u32 | ((green as u32) << 8) | ((blue as u32) << 16))
    }

    unsafe extern "system" fn fixture_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                let blocks = [
                    (
                        RECT {
                            left: 0,
                            top: 0,
                            right: 350,
                            bottom: 700,
                        },
                        rgb(229, 57, 53),
                    ),
                    (
                        RECT {
                            left: 350,
                            top: 0,
                            right: 700,
                            bottom: 700,
                        },
                        rgb(30, 136, 229),
                    ),
                    (
                        RECT {
                            left: 700,
                            top: 0,
                            right: 1050,
                            bottom: 700,
                        },
                        rgb(67, 160, 71),
                    ),
                ];
                for (rect, color) in blocks {
                    let brush = CreateSolidBrush(color);
                    FillRect(dc, &rect, brush);
                    let _ = DeleteObject(HGDIOBJ(brush.0));
                }

                let white = GetStockObject(WHITE_BRUSH);
                let previous = SelectObject(dc, white);
                for x in (20..1030).step_by(20) {
                    let _ = MoveToEx(dc, x, 0, None);
                    let _ = LineTo(dc, x, 700);
                }
                for y in (20..700).step_by(20) {
                    let _ = MoveToEx(dc, 0, y, None);
                    let _ = LineTo(dc, 1050, y);
                }
                let _ = SelectObject(dc, previous);
                let _ = SetBkMode(dc, TRANSPARENT);
                let _ = SetTextColor(dc, rgb(255, 255, 255));
                let title: Vec<u16> = "RED  /  BLUE  /  GREEN — thin grid and text must blur"
                    .encode_utf16()
                    .collect();
                let _ = TextOutW(dc, 36, 48, &title);
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 == 0x1b => {
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

    unsafe extern "system" fn acrylic_proc(
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
            WM_KEYDOWN if wparam.0 == 0x1b => {
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

    unsafe fn register_window_class(
        instance: HINSTANCE,
        class_name: windows::core::PCWSTR,
        proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        background: HBRUSH,
    ) -> WinResult<()> {
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: background,
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        Ok(())
    }

    unsafe fn create_dispatcher() -> WinResult<windows::System::DispatcherQueueController> {
        let dispatcher = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_ASTA,
        })?;
        eprintln!("[poc] dispatcher_queue=created current_thread ASTA");
        Ok(dispatcher)
    }

    unsafe fn create_composition(
        hwnd: HWND,
        create_target: bool,
        create_root: bool,
        state: &mut PocState,
    ) -> WinResult<()> {
        let compositor = Compositor::new()?;
        eprintln!("[poc] compositor=created");
        if !create_target {
            state.compositor = Some(compositor);
            return Ok(());
        }
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
        eprintln!("[poc] composition_target=DesktopWindowTarget top_level=true");
        let root = if create_root {
            let root = compositor.CreateContainerVisual()?;
            target.SetRoot(&root)?;
            eprintln!("[poc] composition_root=ContainerVisual");
            eprintln!("[poc] composition_root_content=empty-no-fill");
            Some(root)
        } else {
            eprintln!("[poc] composition_root=<not-set-negative-control>");
            None
        };
        let composition_target: windows::UI::Composition::CompositionTarget = target.cast()?;
        eprintln!(
            "[poc] target_identity=desktop:{:p} composition:{:p}",
            Interface::as_raw(&target),
            Interface::as_raw(&composition_target)
        );
        state.compositor = Some(compositor);
        state.target = Some(target);
        state.composition_target = Some(composition_target);
        state.root = root;
        Ok(())
    }

    unsafe fn enable_host_backdrop(hwnd: HWND) -> WinResult<()> {
        let enabled = BOOL(1);
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_HOSTBACKDROPBRUSH,
            &enabled as *const _ as *const c_void,
            size_of::<BOOL>() as u32,
        )?;
        eprintln!("[poc] DWMWA_USE_HOSTBACKDROPBRUSH=true");
        Ok(())
    }

    fn create_configuration() -> WinResult<SystemBackdropConfiguration> {
        let configuration = SystemBackdropConfiguration::new()?;
        configuration.SetIsInputActive(true)?;
        configuration.SetTheme(SystemBackdropTheme::Dark)?;
        eprintln!("[poc] system_backdrop_configuration=input-active theme=dark");
        Ok(configuration)
    }

    fn create_controller() -> WinResult<DesktopAcrylicController> {
        let supported = DesktopAcrylicController::IsSupported()?;
        eprintln!("[poc] desktop_acrylic_supported={supported}");
        if !supported {
            return Err(windows::core::Error::new(
                HRESULT(0x80004001_u32 as i32),
                "DesktopAcrylicController reports unsupported",
            ));
        }
        let controller = DesktopAcrylicController::new()?;
        eprintln!("[poc] desktop_acrylic_controller=created");
        Ok(controller)
    }

    fn attach_controller(hwnd: HWND, state: &mut PocState) -> WinResult<()> {
        if state.target_attached {
            return Ok(());
        }
        let controller = state.controller.as_ref().ok_or_else(|| {
            windows::core::Error::new(HRESULT(0x80004005_u32 as i32), "controller missing")
        })?;
        let target = state.composition_target.as_ref().ok_or_else(|| {
            windows::core::Error::new(
                HRESULT(0x80004005_u32 as i32),
                "projected composition target missing",
            )
        })?;
        let window_id = WindowId {
            Value: hwnd.0 as usize as u64,
        };
        eprintln!(
            "[poc] target_mapping=acrylic_hwnd:{:#x} window_id:{:#x} target_hwnd:{:#x} is_top_level:true",
            hwnd.0 as usize, window_id.Value, hwnd.0 as usize
        );
        let attached = controller.SetTargetWithWindowId(window_id, target)?;
        eprintln!("[poc] DesktopAcrylicController.SetTarget_returned={attached}");
        if !attached {
            return Err(windows::core::Error::new(
                HRESULT(0x80004005_u32 as i32),
                "DesktopAcrylicController rejected the Win32 composition target",
            ));
        }
        state.window_id = Some(window_id);
        state.target_attached = true;
        Ok(())
    }

    fn detach_controller(state: &mut PocState) -> WinResult<()> {
        if !state.target_attached {
            return Ok(());
        }
        state
            .controller
            .as_ref()
            .ok_or_else(|| {
                windows::core::Error::new(HRESULT(0x80004005_u32 as i32), "controller missing")
            })?
            .RemoveAllSystemBackdropTargets()?;
        state.target_attached = false;
        Ok(())
    }

    unsafe fn capture_screen_bmp(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        path: &Path,
    ) -> Result<(), String> {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err("GetDC(NULL) failed".into());
        }
        let memory_dc = CreateCompatibleDC(Some(screen_dc));
        if memory_dc.is_invalid() {
            let _ = ReleaseDC(None, screen_dc);
            return Err("CreateCompatibleDC failed".into());
        }
        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(memory_dc);
            let _ = ReleaseDC(None, screen_dc);
            return Err("CreateCompatibleBitmap failed".into());
        }
        let previous = SelectObject(memory_dc, HGDIOBJ(bitmap.0));
        let copy_result = BitBlt(
            memory_dc,
            0,
            0,
            width,
            height,
            Some(screen_dc),
            x,
            y,
            SRCCOPY,
        );

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0_u8; width as usize * height as usize * 4];
        let scanlines = if copy_result.is_ok() {
            GetDIBits(
                memory_dc,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };

        let _ = SelectObject(memory_dc, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory_dc);
        let _ = ReleaseDC(None, screen_dc);

        if scanlines != height {
            return Err(format!(
                "screen capture returned {scanlines} of {height} scanlines"
            ));
        }

        let pixel_offset = 14_u32 + 40;
        let file_size = pixel_offset + pixels.len() as u32;
        let mut file = File::create(path).map_err(|error| error.to_string())?;
        file.write_all(b"BM").map_err(|error| error.to_string())?;
        file.write_all(&file_size.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&[0_u8; 4])
            .map_err(|error| error.to_string())?;
        file.write_all(&pixel_offset.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&40_u32.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&width.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&(-height).to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&1_u16.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&32_u16.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&BI_RGB.0.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&(pixels.len() as u32).to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&[0_u8; 16])
            .map_err(|error| error.to_string())?;
        file.write_all(&pixels).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn write_bmp(path: &Path, width: i32, height: i32, pixels: &[u8]) -> Result<(), String> {
        let pixel_offset = 14_u32 + 40;
        let file_size = pixel_offset + pixels.len() as u32;
        let mut file = File::create(path).map_err(|error| error.to_string())?;
        file.write_all(b"BM").map_err(|error| error.to_string())?;
        file.write_all(&file_size.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&[0_u8; 4])
            .map_err(|error| error.to_string())?;
        file.write_all(&pixel_offset.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&40_u32.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&width.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&(-height).to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&1_u16.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&32_u16.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&BI_RGB.0.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&(pixels.len() as u32).to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&[0_u8; 16])
            .map_err(|error| error.to_string())?;
        file.write_all(pixels).map_err(|error| error.to_string())?;
        Ok(())
    }

    unsafe fn capture_composited_window_bmp(hwnd: HWND, path: &Path) -> Result<(), String> {
        let mut d3d_device: Option<ID3D11Device> = None;
        let mut d3d_context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d_device),
            None,
            Some(&mut d3d_context),
        )
        .map_err(|error| format!("D3D11CreateDevice: {error}"))?;
        let d3d_device = d3d_device.ok_or("D3D11CreateDevice returned no device")?;
        let d3d_context = d3d_context.ok_or("D3D11CreateDevice returned no context")?;
        let dxgi_device: IDXGIDevice = d3d_device
            .cast()
            .map_err(|error| format!("ID3D11Device -> IDXGIDevice: {error}"))?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)
            .map_err(|error| format!("CreateDirect3D11DeviceFromDXGIDevice: {error}"))?;
        let direct3d_device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|error| format!("IInspectable -> IDirect3DDevice: {error}"))?;

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|error| format!("GraphicsCaptureItem factory: {error}"))?;
        let item: GraphicsCaptureItem = interop
            .CreateForWindow(hwnd)
            .map_err(|error| format!("CreateForWindow: {error}"))?;
        let size = item
            .Size()
            .map_err(|error| format!("GraphicsCaptureItem.Size: {error}"))?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|error| format!("CreateFreeThreaded: {error}"))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| format!("CreateCaptureSession: {error}"))?;
        let _ = session.SetIsBorderRequired(false);
        session
            .StartCapture()
            .map_err(|error| format!("StartCapture: {error}"))?;

        let deadline = Instant::now() + Duration::from_secs(5);
        let frame = loop {
            if let Ok(frame) = pool.TryGetNextFrame() {
                break frame;
            }
            if Instant::now() >= deadline {
                return Err("Windows Graphics Capture produced no frame in 5 seconds".into());
            }
            pump_for(Duration::from_millis(50));
        };
        let frame_size = frame
            .ContentSize()
            .map_err(|error| format!("frame.ContentSize: {error}"))?;
        let surface = frame
            .Surface()
            .map_err(|error| format!("frame.Surface: {error}"))?;
        let access: IDirect3DDxgiInterfaceAccess = surface
            .cast()
            .map_err(|error| format!("surface interface access: {error}"))?;
        let source: ID3D11Texture2D = access
            .GetInterface()
            .map_err(|error| format!("surface texture: {error}"))?;
        let mut description = D3D11_TEXTURE2D_DESC::default();
        source.GetDesc(&mut description);
        description.BindFlags = 0;
        description.MiscFlags = 0;
        description.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        description.Usage = D3D11_USAGE_STAGING;
        let mut staging: Option<ID3D11Texture2D> = None;
        d3d_device
            .CreateTexture2D(&description, None, Some(&mut staging))
            .map_err(|error| format!("CreateTexture2D staging: {error}"))?;
        let staging = staging.ok_or("CreateTexture2D returned no staging texture")?;
        d3d_context.CopyResource(&staging, &source);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        d3d_context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|error| format!("Map staging texture: {error}"))?;

        let width = frame_size.Width.max(0) as usize;
        let height = frame_size.Height.max(0) as usize;
        let row_bytes = width * 4;
        let mut pixels = vec![0_u8; row_bytes * height];
        for row in 0..height {
            let source_row = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                row_bytes,
            );
            pixels[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(source_row);
        }
        d3d_context.Unmap(&staging, 0);
        let _ = frame.Close();
        let _ = session.Close();
        let _ = pool.Close();
        write_bmp(path, width as i32, height as i32, &pixels)
    }

    unsafe fn pump_for(duration: Duration) {
        let deadline = Instant::now() + duration;
        let mut message = MSG::default();
        while Instant::now() < deadline {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    pub unsafe fn run() -> WinResult<()> {
        let mut self_contained = false;
        let mut capture_path = None;
        let mut qa_variant = QaVariant::Full;
        let mut qa_variant_requested = false;
        let mut qa_seconds = 4_u64;
        let mut qa_manual = false;
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--self-contained" => self_contained = true,
                "--qa-manual" => {
                    qa_manual = true;
                    self_contained = true;
                    qa_variant = QaVariant::Full;
                }
                "--qa-capture" => {
                    capture_path = Some(match arguments.next() {
                        Some(path) => path,
                        None => "native-acrylic-gate-a.bmp".into(),
                    });
                }
                "--qa-variant" => {
                    let value = arguments.next().ok_or_else(|| {
                        windows::core::Error::new(
                            HRESULT(0x80070057_u32 as i32),
                            "--qa-variant requires a value",
                        )
                    })?;
                    qa_variant = QaVariant::parse(&value).ok_or_else(|| {
                        windows::core::Error::new(
                            HRESULT(0x80070057_u32 as i32),
                            format!("unknown QA variant: {value}"),
                        )
                    })?;
                    qa_variant_requested = true;
                }
                "--qa-seconds" => {
                    let value = arguments.next().ok_or_else(|| {
                        windows::core::Error::new(
                            HRESULT(0x80070057_u32 as i32),
                            "--qa-seconds requires a value",
                        )
                    })?;
                    qa_seconds = value.parse().map_err(|_| {
                        windows::core::Error::new(
                            HRESULT(0x80070057_u32 as i32),
                            "--qa-seconds must be an unsigned integer",
                        )
                    })?;
                }
                _ => {}
            }
        }
        if qa_manual {
            capture_path = None;
            qa_variant_requested = false;
            eprintln!("[poc] visual_qa=manual");
            eprintln!("[poc] runtime=self-contained");
        }
        eprintln!("[poc] qa_variant={qa_variant:?}");

        let _self_contained_runtime;
        let _bootstrap;
        if self_contained {
            _self_contained_runtime = Some(initialize_self_contained()?);
            _bootstrap = None;
        } else {
            _self_contained_runtime = None;
            _bootstrap = Some(initialize_bootstrap()?);
        }

        RoInitialize(RO_INIT_SINGLETHREADED)?;
        eprintln!("[poc] apartment=single_threaded");

        let mut state = PocState::new();
        if qa_variant.needs_dispatcher() {
            state.dispatcher = Some(create_dispatcher()?);
        }

        let module = GetModuleHandleW(None)?;
        let instance = HINSTANCE(module.0);
        register_window_class(
            instance,
            FIXTURE_CLASS,
            fixture_proc,
            HBRUSH(GetStockObject(WHITE_BRUSH).0),
        )?;
        register_window_class(instance, ACRYLIC_CLASS, acrylic_proc, HBRUSH::default())?;
        eprintln!("[poc] test_window_class_background=null");
        eprintln!("[poc] erase_background=disabled");

        let fixture = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            FIXTURE_CLASS,
            windows::core::w!("Acrylic visual fixture"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            120,
            100,
            1060,
            740,
            None,
            None,
            Some(instance),
            None,
        )?;
        let _ = ShowWindow(fixture, SW_SHOW);

        let acrylic = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            ACRYLIC_CLASS,
            windows::core::w!("DesktopAcrylicController proof"),
            WS_OVERLAPPEDWINDOW,
            330,
            250,
            640,
            420,
            None,
            None,
            Some(instance),
            None,
        )?;
        let class_background = GetClassLongPtrW(acrylic, GCLP_HBRBACKGROUND);
        let ex_style = GetWindowLongPtrW(acrylic, GWL_EXSTYLE) as u32;
        eprintln!("[poc] test_window_class_background_value={class_background:#x}");
        eprintln!(
            "[poc] test_window_exstyle={ex_style:#010x} no_redirection_bitmap={}",
            ex_style & WS_EX_NOREDIRECTIONBITMAP.0 != 0
        );
        eprintln!(
            "[poc] fixture_hwnd={:#x} acrylic_hwnd={:#x} host=top-level",
            fixture.0 as usize, acrylic.0 as usize
        );

        if qa_variant.needs_target() {
            enable_host_backdrop(acrylic)?;
        }
        if qa_variant.needs_compositor() {
            create_composition(
                acrylic,
                qa_variant.needs_target(),
                qa_variant != QaVariant::FullNoRoot,
                &mut state,
            )?;
        }
        if qa_variant.needs_configuration() {
            state.configuration = Some(create_configuration()?);
        }
        if qa_variant.needs_controller() {
            state.controller = Some(create_controller()?);
        }
        if qa_variant.needs_configuration() && qa_variant != QaVariant::TargetThenConfig {
            state
                .controller
                .as_ref()
                .ok_or_else(|| {
                    windows::core::Error::new(HRESULT(0x80004005_u32 as i32), "controller missing")
                })?
                .SetSystemBackdropConfiguration(state.configuration.as_ref().ok_or_else(
                    || {
                        windows::core::Error::new(
                            HRESULT(0x80004005_u32 as i32),
                            "configuration missing",
                        )
                    },
                )?)?;
            eprintln!("[poc] configuration_set=before-target-if-present");
        }
        if qa_variant.needs_set_target() {
            attach_controller(acrylic, &mut state)?;
        }
        if qa_variant == QaVariant::TargetThenConfig {
            state
                .controller
                .as_ref()
                .ok_or_else(|| {
                    windows::core::Error::new(HRESULT(0x80004005_u32 as i32), "controller missing")
                })?
                .SetSystemBackdropConfiguration(state.configuration.as_ref().ok_or_else(
                    || {
                        windows::core::Error::new(
                            HRESULT(0x80004005_u32 as i32),
                            "configuration missing",
                        )
                    },
                )?)?;
            eprintln!("[poc] configuration_order=target-then-configuration");
        }
        if qa_manual {
            SetWindowTextW(
                acrylic,
                windows::core::w!("Acrylic ON — A: Acrylic, T: Transparent, Esc: Exit"),
            )?;
            eprintln!("[poc] root_visual=attached");
            eprintln!("[poc] controller=created");
            eprintln!("[poc] configuration=active");
            eprintln!("[poc] set_target=true");
            eprintln!("[poc] controls=A:Acrylic_ON T:Transparent_OFF Esc:Exit");
            eprintln!("[poc] waiting_for_human_visual_verification=true");
        }
        let _ = ShowWindow(acrylic, SW_SHOW);
        eprintln!("[poc] READY: variant entered message pump; press Escape to exit");

        if let Some(path) = capture_path {
            pump_for(Duration::from_secs(3));
            if let Err(screen_error) = capture_screen_bmp(100, 80, 1120, 800, Path::new(&path)) {
                eprintln!("[poc] qa_screen_capture=unavailable reason={screen_error}");
                capture_composited_window_bmp(acrylic, Path::new(&path)).map_err(|message| {
                    windows::core::Error::new(HRESULT(0x80004005_u32 as i32), message)
                })?;
                eprintln!("[poc] qa_capture_source=Windows.Graphics.Capture(HWND)");
            }
            eprintln!("[poc] qa_capture={path}");
            state.shutdown();
            return Ok(());
        }

        if qa_variant_requested {
            pump_for(Duration::from_secs(qa_seconds));
            eprintln!("[poc] qa_variant_stable_seconds={qa_seconds}");
            state.shutdown();
            return Ok(());
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if qa_manual && message.hwnd == acrylic && message.message == WM_KEYDOWN {
                match message.wParam.0 {
                    0x41 => {
                        attach_controller(acrylic, &mut state)?;
                        SetWindowTextW(
                            acrylic,
                            windows::core::w!("Acrylic ON — A: Acrylic, T: Transparent, Esc: Exit"),
                        )?;
                        eprintln!("[poc] comparison=acrylic-on");
                        continue;
                    }
                    0x54 => {
                        detach_controller(&mut state)?;
                        SetWindowTextW(
                            acrylic,
                            windows::core::w!(
                                "Transparent — A: Acrylic, T: Transparent, Esc: Exit"
                            ),
                        )?;
                        eprintln!("[poc] comparison=transparent-acrylic-off");
                        continue;
                    }
                    _ => {}
                }
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        state.shutdown();
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn main() {
    if let Err(error) = unsafe { windows_poc::run() } {
        eprintln!(
            "[poc] FAILED HRESULT={:#010x} message={}",
            error.code().0 as u32,
            error.message()
        );
        std::process::exit(1);
    }
}
