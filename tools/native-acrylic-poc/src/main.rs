#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("native-acrylic-poc only runs on Windows");
}

#[cfg(target_os = "windows")]
mod windows_poc {
    use std::{
        ffi::c_void,
        fs::File,
        io::Write,
        mem::size_of,
        path::Path,
        time::{Duration, Instant},
    };
    use windows::{
        core::{IInspectable, Interface, Result as WinResult, BOOL, HRESULT, HSTRING},
        Win32::{
            Foundation::{
                COLORREF, FARPROC, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM,
            },
            Graphics::{
                Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH},
                Gdi::{
                    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
                    CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, GetDC, GetDIBits,
                    GetStockObject, LineTo, MoveToEx, ReleaseDC, SelectObject, SetBkMode,
                    SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
                    HBRUSH, HGDIOBJ, PAINTSTRUCT, SRCCOPY, TRANSPARENT, WHITE_BRUSH,
                },
            },
            System::{
                LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
                WinRT::{
                    Composition::ICompositorDesktopInterop, CreateDispatcherQueueController,
                    DispatcherQueueOptions, RoActivateInstance, RoInitialize, DQTAT_COM_ASTA,
                    DQTYPE_THREAD_CURRENT, RO_INIT_SINGLETHREADED,
                },
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, LoadCursorW,
                PeekMessageW, PostQuitMessage, RegisterClassW, ShowWindow, TranslateMessage,
                CS_HREDRAW, CS_VREDRAW, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE,
                WM_DESTROY, WM_KEYDOWN, WM_PAINT, WM_QUIT, WNDCLASSW, WS_OVERLAPPEDWINDOW,
                WS_VISIBLE,
            },
        },
        UI::Composition::{Compositor, Desktop::DesktopWindowTarget},
    };
    use windows_core::imp::define_interface;

    const FIXTURE_CLASS: windows::core::PCWSTR = windows::core::w!("AlanAcrylicFixture");
    const ACRYLIC_CLASS: windows::core::PCWSTR = windows::core::w!("AlanAcrylicTarget");
    const DESKTOP_ACRYLIC_RUNTIME_CLASS: &str =
        "Microsoft.UI.Composition.SystemBackdrops.DesktopAcrylicController";
    const WINDOWS_APP_SDK_MAJOR_MINOR: u32 = 0x0001_0008;
    const WINDOWS_APP_RUNTIME_MIN_VERSION: u64 = 0x1f40_03b2_06a5_0000;

    type BootstrapInitialize =
        unsafe extern "system" fn(u32, windows::core::PCWSTR, u64) -> HRESULT;
    type BootstrapShutdown = unsafe extern "system" fn();

    struct BootstrapLifetime {
        _module: HMODULE,
        shutdown: BootstrapShutdown,
    }

    impl Drop for BootstrapLifetime {
        fn drop(&mut self) {
            unsafe { (self.shutdown)() };
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WindowId {
        value: u64,
    }

    define_interface!(
        ISystemBackdropController,
        ISystemBackdropController_Vtbl,
        0x5632d76c_0b74_5b52_aa33_80262068aeb2
    );
    windows_core::imp::interface_hierarchy!(
        ISystemBackdropController,
        windows_core::IUnknown,
        IInspectable
    );

    #[repr(C)]
    pub struct ISystemBackdropController_Vtbl {
        base__: windows_core::IInspectable_Vtbl,
        set_target_with_window_id:
            unsafe extern "system" fn(*mut c_void, WindowId, *mut c_void, *mut bool) -> HRESULT,
        set_target_with_core_window: usize,
    }

    impl ISystemBackdropController {
        unsafe fn set_target(&self, hwnd: HWND, target: &DesktopWindowTarget) -> WinResult<bool> {
            let mut result = false;
            (Interface::vtable(self).set_target_with_window_id)(
                Interface::as_raw(self),
                WindowId {
                    value: hwnd.0 as usize as u64,
                },
                Interface::as_raw(target),
                &mut result,
            )
            .ok()?;
            Ok(result)
        }
    }

    struct CompositionLifetime {
        _dispatcher: windows::System::DispatcherQueueController,
        _compositor: Compositor,
        _target: DesktopWindowTarget,
        _controller: ISystemBackdropController,
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

    unsafe fn initialize_bootstrap() -> WinResult<BootstrapLifetime> {
        let module = LoadLibraryW(windows::core::w!(
            "Microsoft.WindowsAppRuntime.Bootstrap.dll"
        ))?;
        let initialize: BootstrapInitialize =
            required_export(module, windows::core::s!("MddBootstrapInitialize"))?;
        let shutdown: BootstrapShutdown =
            required_export(module, windows::core::s!("MddBootstrapShutdown"))?;
        initialize(
            WINDOWS_APP_SDK_MAJOR_MINOR,
            windows::core::w!(""),
            WINDOWS_APP_RUNTIME_MIN_VERSION,
        )
        .ok()?;
        eprintln!("[poc] windows_app_sdk=1.8 runtime_min=8000.946.1701.0 bootstrap=initialized");
        Ok(BootstrapLifetime {
            _module: module,
            shutdown,
        })
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

    unsafe fn enable_acrylic(hwnd: HWND) -> WinResult<CompositionLifetime> {
        let dispatcher = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_ASTA,
        })?;
        eprintln!("[poc] dispatcher_queue=created current_thread ASTA");

        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
        eprintln!("[poc] composition_target=DesktopWindowTarget top_level=true");

        let enabled = BOOL(1);
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_HOSTBACKDROPBRUSH,
            &enabled as *const _ as *const c_void,
            size_of::<BOOL>() as u32,
        )?;
        eprintln!("[poc] DWMWA_USE_HOSTBACKDROPBRUSH=true");

        let inspectable = RoActivateInstance(&HSTRING::from(DESKTOP_ACRYLIC_RUNTIME_CLASS))?;
        let controller: ISystemBackdropController = inspectable.cast()?;
        let attached = controller.set_target(hwnd, &target)?;
        eprintln!("[poc] DesktopAcrylicController.SetTarget={attached}");
        if !attached {
            return Err(windows::core::Error::new(
                HRESULT(0x80004005_u32 as i32),
                "DesktopAcrylicController rejected the Win32 composition target",
            ));
        }

        Ok(CompositionLifetime {
            _dispatcher: dispatcher,
            _compositor: compositor,
            _target: target,
            _controller: controller,
        })
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
        RoInitialize(RO_INIT_SINGLETHREADED)?;
        eprintln!("[poc] apartment=single_threaded");
        let _bootstrap = initialize_bootstrap()?;

        let module = GetModuleHandleW(None)?;
        let instance = HINSTANCE(module.0);
        register_window_class(
            instance,
            FIXTURE_CLASS,
            fixture_proc,
            HBRUSH(GetStockObject(WHITE_BRUSH).0),
        )?;
        register_window_class(instance, ACRYLIC_CLASS, acrylic_proc, HBRUSH::default())?;

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
            WINDOW_EX_STYLE::default(),
            ACRYLIC_CLASS,
            windows::core::w!("DesktopAcrylicController proof"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            330,
            250,
            640,
            420,
            None,
            None,
            Some(instance),
            None,
        )?;
        let _ = ShowWindow(acrylic, SW_SHOW);
        eprintln!(
            "[poc] fixture_hwnd={:#x} acrylic_hwnd={:#x} host=top-level",
            fixture.0 as usize, acrylic.0 as usize
        );

        let _composition = enable_acrylic(acrylic)?;
        eprintln!(
            "[poc] READY: visually inspect or capture the acrylic window; press Escape to exit"
        );

        let mut arguments = std::env::args().skip(1);
        if arguments.next().as_deref() == Some("--qa-capture") {
            let path = arguments
                .next()
                .unwrap_or_else(|| "native-acrylic-gate-a.bmp".into());
            pump_for(Duration::from_secs(3));
            capture_screen_bmp(100, 80, 1120, 800, Path::new(&path)).map_err(|message| {
                windows::core::Error::new(HRESULT(0x80004005_u32 as i32), message)
            })?;
            eprintln!("[poc] qa_capture={path}");
            return Ok(());
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
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
