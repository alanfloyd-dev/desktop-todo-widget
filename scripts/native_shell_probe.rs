//! Dependency-free Windows Shell probe for Phase 1 diagnostics.
//!
//! Build and run directly:
//! `rustc scripts/native_shell_probe.rs -o native-shell-probe.exe`

#![cfg(target_os = "windows")]

type Hwnd = isize;
type Bool = i32;
type Lparam = isize;

const PROGMAN_SPAWN_WORKERW: u32 = 0x052C;
const SMTO_ABORTIFHUNG: u32 = 0x0002;

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(Hwnd, Lparam) -> Bool, lparam: Lparam)
        -> Bool;
    fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
    fn FindWindowExW(
        parent: Hwnd,
        child_after: Hwnd,
        class_name: *const u16,
        window_name: *const u16,
    ) -> Hwnd;
    fn GetClassNameW(hwnd: Hwnd, class_name: *mut u16, max_count: i32) -> i32;
    fn SendMessageTimeoutW(
        hwnd: Hwnd,
        message: u32,
        wparam: usize,
        lparam: isize,
        flags: u32,
        timeout: u32,
        result: *mut usize,
    ) -> isize;
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn class_name(hwnd: Hwnd) -> String {
    let mut buffer = [0_u16; 128];
    // SAFETY: `buffer` is writable for the exact capacity passed and the HWND
    // is used only as an opaque query target.
    let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 0 {
        "unknown".into()
    } else {
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

struct Search {
    def_view_host: Hwnd,
    worker_after_def_view: Hwnd,
}

unsafe extern "system" fn enumerate(top_level: Hwnd, lparam: Lparam) -> Bool {
    // SAFETY: EnumWindows supplies a valid top-level HWND and `lparam` points
    // to the caller-owned Search for the synchronous enumeration lifetime.
    let shell_view = wide("SHELLDLL_DefView");
    let worker_class = wide("WorkerW");
    let def_view = FindWindowExW(top_level, 0, shell_view.as_ptr(), std::ptr::null());
    if def_view != 0 {
        let search = &mut *(lparam as *mut Search);
        search.def_view_host = top_level;
        search.worker_after_def_view =
            FindWindowExW(0, top_level, worker_class.as_ptr(), std::ptr::null());
        return 0;
    }
    1
}

unsafe extern "system" fn enumerate_shell_classes(top_level: Hwnd, lparam: Lparam) -> Bool {
    // SAFETY: `lparam` points to the caller-owned counter for the full
    // synchronous EnumWindows call and is never retained.
    let count = &mut *(lparam as *mut usize);
    let name = class_name(top_level);
    if *count < 32
        || name.eq_ignore_ascii_case("Progman")
        || name.eq_ignore_ascii_case("WorkerW")
        || name.eq_ignore_ascii_case("SHELLDLL_DefView")
    {
        println!("TopLevelShellWindow=0x{top_level:X} class={name}");
    }
    *count += 1;
    1
}

fn main() {
    let progman_class = wide("Progman");
    let worker_class = wide("WorkerW");
    // SAFETY: the null-terminated class buffer remains alive for the
    // synchronous lookup and Win32 retains no pointer.
    let progman = unsafe { FindWindowW(progman_class.as_ptr(), std::ptr::null()) };
    if progman == 0 {
        eprintln!("ERROR Progman not found");
        let mut count = 0_usize;
        // SAFETY: EnumWindows is synchronous, so the pointer to `count`
        // remains valid and exclusively borrowed throughout callback use.
        unsafe {
            EnumWindows(
                enumerate_shell_classes,
                (&mut count as *mut usize) as Lparam,
            );
        }
        eprintln!("TopLevelWindowCount={count}");
        std::process::exit(1);
    }

    let mut message_result = 0_usize;
    // SAFETY: Progman is non-null and `message_result` is writable for the
    // bounded SendMessageTimeoutW call.
    let message_ok = unsafe {
        SendMessageTimeoutW(
            progman,
            PROGMAN_SPAWN_WORKERW,
            0xD,
            0x1,
            SMTO_ABORTIFHUNG,
            1_000,
            &mut message_result,
        )
    };
    // SAFETY: both class buffers remain alive and Progman was validated above.
    let child_worker = unsafe {
        FindWindowExW(progman, 0, worker_class.as_ptr(), std::ptr::null())
    };
    let mut search = Search {
        def_view_host: 0,
        worker_after_def_view: 0,
    };
    // SAFETY: EnumWindows is synchronous, so the pointer to `search` remains
    // valid and exclusively borrowed until enumeration completes.
    unsafe {
        EnumWindows(enumerate, (&mut search as *mut Search) as Lparam);
    }

    println!("Progman=0x{progman:X} class={}", class_name(progman));
    println!(
        "SpawnMessage={} result={message_result}",
        if message_ok != 0 { "ok" } else { "failed" }
    );
    println!(
        "WorkerWChild=0x{child_worker:X} class={}",
        class_name(child_worker)
    );
    println!(
        "DefViewHost=0x{:X} class={}",
        search.def_view_host,
        class_name(search.def_view_host)
    );
    println!(
        "WorkerAfterDefView=0x{:X} class={}",
        search.worker_after_def_view,
        class_name(search.worker_after_def_view)
    );

    if child_worker != 0 {
        println!("SelectedStrategy=workerw-child-of-progman");
    } else if search.worker_after_def_view != 0 {
        println!("SelectedStrategy=workerw-sibling-of-defview");
    } else {
        println!("SelectedStrategy=progman-fallback");
    }
}
