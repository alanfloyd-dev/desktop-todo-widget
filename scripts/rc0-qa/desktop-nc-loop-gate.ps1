# v1 Desktop usability gate: which non-client loops actually work for which
# window shape?
#
# Tauri's `start_dragging` and its undecorated resize borders both end in
# `WM_NCLBUTTONDOWN` with a hit-test code, and let user32 run the modal
# move/size loop. This harness builds the candidate window shapes, drives a real
# held button (pressed over a topmost window this script owns, so no click can
# reach another application), posts the non-client press, walks the cursor, and
# reports whether the window moved or resized.
#
# Shapes: top-level with caption (control), and WS_CHILD variants with and
# without WS_CAPTION / WS_THICKFRAME.

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class V1Loop {
  public delegate IntPtr WndProcD(IntPtr h, uint m, IntPtr w, IntPtr l);
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct WNDCLASS { public uint style; public WndProcD lpfnWndProc; public int cbClsExtra; public int cbWndExtra; public IntPtr hInstance; public IntPtr hIcon; public IntPtr hCursor; public IntPtr hbrBackground; public string lpszMenuName; public string lpszClassName; }
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rr, B; }
  [StructLayout(LayoutKind.Sequential)] public struct P { public int X, Y; }
  [StructLayout(LayoutKind.Sequential)] public struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam; public IntPtr lParam; public uint time; public P pt; public uint lPrivate; }

  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern ushort RegisterClassW(ref WNDCLASS c);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr CreateWindowExW(uint ex, string cls, string name, uint style, int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);
  [DllImport("user32.dll")] public static extern IntPtr DefWindowProcW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr GetModuleHandleW(string n);
  [DllImport("user32.dll")] public static extern bool PeekMessageW(out MSG msg, IntPtr h, uint min, uint max, uint remove);
  [DllImport("user32.dll")] public static extern bool TranslateMessage(ref MSG msg);
  [DllImport("user32.dll")] public static extern IntPtr DispatchMessageW(ref MSG msg);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool DestroyWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);

  const uint WS_POPUP = 0x80000000, WS_CHILD = 0x40000000, WS_VISIBLE = 0x10000000;
  const uint WS_CAPTION = 0x00C00000, WS_THICKFRAME = 0x00040000, WS_CLIPSIBLINGS = 0x04000000, WS_TABSTOP = 0x00010000;
  const uint WS_EX_TOPMOST = 0x00000008, WS_EX_TOOLWINDOW = 0x00000080;
  const uint WM_NCLBUTTONDOWN = 0x00A1, WM_LBUTTONUP = 0x0202, PM_REMOVE = 0x0001;
  const int HTCAPTION = 2, HTBOTTOMRIGHT = 17;

  static string cls = "V1LoopClass";
  static bool done;
  static int padX = 40, padY = 60, padW = 180, padH = 90;

  public static string Run() {
    var report = new StringBuilder();
    IntPtr inst = GetModuleHandleW(null);
    WNDCLASS wc = new WNDCLASS();
    wc.lpfnWndProc = (ph, pm, pw, pl) => DefWindowProcW(ph, pm, pw, pl);
    wc.hInstance = inst; wc.lpszClassName = cls;
    RegisterClassW(ref wc);

    // Topmost pad: the real button is pressed here, then the cursor moves to the
    // window under test. Nothing else can receive the press.
    IntPtr pad = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, cls, "pad",
      WS_POPUP | WS_VISIBLE, padX, padY, padW, padH, IntPtr.Zero, IntPtr.Zero, inst, IntPtr.Zero);

    IntPtr shell = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, cls, "shell",
      WS_POPUP | WS_VISIBLE, 40, 200, 360, 320, IntPtr.Zero, IntPtr.Zero, inst, IntPtr.Zero);

    IntPtr cPlain = CreateWindowExW(0, cls, "cPlain", WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_TABSTOP, 10, 10, 200, 80, shell, IntPtr.Zero, inst, IntPtr.Zero);
    IntPtr cCaption = CreateWindowExW(0, cls, "cCaption", WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CAPTION, 10, 110, 200, 80, shell, IntPtr.Zero, inst, IntPtr.Zero);
    IntPtr cThick = CreateWindowExW(0, cls, "cThick", WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_THICKFRAME, 10, 220, 200, 80, shell, IntPtr.Zero, inst, IntPtr.Zero);

    Thread.Sleep(600);
    Case(report, "child/plain        move", cPlain, HTCAPTION);
    Case(report, "child/caption      move", cCaption, HTCAPTION);
    Case(report, "child/thickframe   move", cThick, HTCAPTION);
    Case(report, "child/plain        size", cPlain, HTBOTTOMRIGHT);
    Case(report, "child/caption      size", cCaption, HTBOTTOMRIGHT);
    Case(report, "child/thickframe   size", cThick, HTBOTTOMRIGHT);

    DestroyWindow(cPlain); DestroyWindow(cCaption); DestroyWindow(cThick);
    DestroyWindow(shell); DestroyWindow(pad);
    return report.ToString();
  }

  static void Case(StringBuilder report, string label, IntPtr hwnd, int hit) {
    R before; GetWindowRect(hwnd, out before);
    Gesture(hwnd, hit, 70, 50);
    R after; GetWindowRect(hwnd, out after);
    int dx = after.L - before.L, dy = after.T - before.T;
    int dw = (after.Rr - after.L) - (before.Rr - before.L), dh = (after.B - after.T) - (before.B - before.T);
    report.AppendLine(string.Format("{0,-26} pos_delta=({1},{2}) size_delta=({3},{4}) changed={5}",
      label, dx, dy, dw, dh, (dx != 0 || dy != 0 || dw != 0 || dh != 0) ? "YES" : "no"));
  }

  static void Gesture(IntPtr hwnd, int hit, int dx, int dy) {
    R r; GetWindowRect(hwnd, out r);
    int sx = (r.L + r.Rr) / 2, sy = (r.T + r.B) / 2;

    // Press over the pad, then move onto the target while the button is held.
    SetCursorPos(padX + padW / 2, padY + padH / 2);
    Thread.Sleep(150);
    done = false;
    IntPtr lp = (IntPtr)((sy << 16) | (sx & 0xFFFF));
    var t = new Thread(() => {
      mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
      Thread.Sleep(120);
      SetCursorPos(sx, sy);
      Thread.Sleep(120);
      PostMessageW(hwnd, WM_NCLBUTTONDOWN, (IntPtr)hit, lp);
      Thread.Sleep(250);
      for (int i = 1; i <= 16; i++) {
        SetCursorPos(sx + dx * i / 16, sy + dy * i / 16);
        Thread.Sleep(35);
      }
      Thread.Sleep(150);
      PostMessageW(hwnd, WM_LBUTTONUP, IntPtr.Zero, lp);
      mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
      SetCursorPos(padX + padW / 2, padY + padH / 2);
      done = true;
    });
    t.IsBackground = true; t.Start();

    var limit = DateTime.UtcNow.AddSeconds(6);
    while (!done && DateTime.UtcNow < limit) {
      MSG msg;
      while (PeekMessageW(out msg, IntPtr.Zero, 0, 0, PM_REMOVE)) { TranslateMessage(ref msg); DispatchMessageW(ref msg); }
      Thread.Sleep(5);
    }
    t.Join(500);
    Thread.Sleep(250);
  }
}
"@

[void][V1Loop]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))
Write-Host ([V1Loop]::Run())
