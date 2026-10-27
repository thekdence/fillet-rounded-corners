//! Thin, safe wrappers over the Win32 and DWM calls Fillet needs.
//!
//! Windows are passed around as `isize` so they can be hashed and sent
//! between threads; they are only ever turned back into `HWND` here.

use std::{ffi::c_void, mem, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, RECT},
    Graphics::{
        Dwm::{DwmGetColorizationColor, DwmGetWindowAttribute, DwmSetWindowAttribute},
        Gdi::{CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MonitorFromWindow, SetWindowRgn, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    },
    System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION},
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::{
            EnumWindows, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextW,
            GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowArranged, IsWindowVisible, IsZoomed,
            SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
            SWP_NOOWNERZORDER, SWP_NOZORDER, SW_SHOWNOACTIVATE,
        },
    },
};

use crate::layout::Rect;

const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
const DWMWA_CLOAKED: u32 = 14;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;

pub const CORNER_DEFAULT: u32 = 0;
pub const CORNER_ROUND: u32 = 2;
pub const CORNER_ROUND_SMALL: u32 = 3;

pub const COLOR_NONE: u32 = 0xFFFF_FFFE;
pub const COLOR_DEFAULT: u32 = 0xFFFF_FFFF;

const WS_CHILD: u32 = 0x4000_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;

/// Smallest window, in logical pixels, that gets rounded corners.
const MIN_ROUNDED_SIZE: f64 = 48.0;
const WS_EX_APPWINDOW: u32 = 0x0004_0000;

/// Shell and system surfaces that must never be restyled or moved.
const SYSTEM_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "Xaml_WindowedPopupClass",
    "Windows.UI.Core.CoreWindow",
    "ForegroundStaging",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
    "#32768", // menus
    "tooltips_class32",
    "SysShadow",
];

fn h(hwnd: isize) -> HWND {
    hwnd as HWND
}

fn rect(r: RECT) -> Rect {
    Rect::from_edges(r.left, r.top, r.right, r.bottom)
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow() as isize }
}

pub fn exists(hwnd: isize) -> bool {
    unsafe { IsWindow(h(hwnd)) != 0 }
}

pub fn is_maximized(hwnd: isize) -> bool {
    unsafe { IsZoomed(h(hwnd)) != 0 }
}

fn is_snapped(hwnd: isize) -> bool {
    unsafe { IsWindowArranged(h(hwnd)) != 0 }
}

fn style(hwnd: isize) -> u32 {
    unsafe { GetWindowLongW(h(hwnd), GWL_STYLE) as u32 }
}

fn ex_style(hwnd: isize) -> u32 {
    unsafe { GetWindowLongW(h(hwnd), GWL_EXSTYLE) as u32 }
}

pub fn is_resizable(hwnd: isize) -> bool {
    style(hwnd) & WS_THICKFRAME != 0
}

fn is_cloaked(hwnd: isize) -> bool {
    let mut cloaked = 0u32;
    let hr = unsafe { DwmGetWindowAttribute(h(hwnd), DWMWA_CLOAKED, ptr::addr_of_mut!(cloaked).cast(), 4) };
    hr == 0 && cloaked != 0
}

fn class_name(hwnd: isize) -> String {
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(h(hwnd), buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

pub fn title(hwnd: isize) -> String {
    let mut buf = [0u16; 256];
    // For other processes' windows this reads the cached title without
    // sending a message, so a hung app can't stall us.
    let len = unsafe { GetWindowTextW(h(hwnd), buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

pub fn process_id(hwnd: isize) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(h(hwnd), &mut pid) };
    pid
}

/// Lowercase executable name, e.g. `chrome.exe`.
pub fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().map(str::to_lowercase)
    }
}

/// All top-level windows, front to back.
pub fn top_level_windows() -> Vec<isize> {
    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> windows_sys::core::BOOL {
        let list = unsafe { &mut *(list as *mut Vec<isize>) };
        list.push(hwnd as isize);
        1
    }
    let mut list: Vec<isize> = Vec::new();
    unsafe { EnumWindows(Some(collect), ptr::addr_of_mut!(list) as LPARAM) };
    list
}

/// A shown, ordinary window that isn't part of the shell. Minimised counts.
fn is_shown(hwnd: isize) -> bool {
    unsafe {
        if IsWindow(h(hwnd)) == 0 || IsWindowVisible(h(hwnd)) == 0 {
            return false;
        }
    }
    if style(hwnd) & WS_CHILD != 0 || is_cloaked(hwnd) {
        return false;
    }
    !SYSTEM_CLASSES.contains(&class_name(hwnd).as_str())
}

/// On screen right now: shown and not minimised.
fn is_plain_visible(hwnd: isize) -> bool {
    is_shown(hwnd) && unsafe { IsIconic(h(hwnd)) } == 0
}

/// Should this window get rounded corners? Dialogs and tool windows count,
/// maximised and full-screen windows don't.
pub fn wants_corners(hwnd: isize) -> bool {
    if !is_plain_visible(hwnd) || is_maximized(hwnd) {
        return false;
    }
    // Click-through overlays aren't windows you see, and rounding one makes
    // DWM draw a frame around nothing. Apps (Fillet included, via Tauri)
    // park tiny transparent helpers like this at the top-left of the screen.
    if ex_style(hwnd) & WS_EX_TRANSPARENT != 0 || process_id(hwnd) == std::process::id() {
        return false;
    }
    match (window_rect(hwnd), monitor(hwnd)) {
        (Some(win), Some(mon)) => {
            // Too small to be a real window: rounding would turn it into a dot.
            let min = (MIN_ROUNDED_SIZE * mon.scale).round() as i32;
            // A window covering its whole monitor is a game or video player.
            win.w >= min && win.h >= min && !mon.monitor.is_within(&win)
        }
        _ => false,
    }
}

/// Should this window be arranged? Only real app windows: no tool windows,
/// popups or owned dialogs, which would be disruptive to move.
pub fn wants_layout(hwnd: isize) -> bool {
    is_plain_visible(hwnd) && is_app_window(hwnd)
}

/// A window that shows up in the taskbar as an app of its own.
pub fn is_app_window(hwnd: isize) -> bool {
    if !is_shown(hwnd) {
        return false;
    }
    let ex = ex_style(hwnd);
    if ex & WS_EX_TOOLWINDOW != 0 {
        return false;
    }
    let app_window = ex & WS_EX_APPWINDOW != 0;
    if app_window {
        return true;
    }
    let owned = unsafe { !GetWindow(h(hwnd), GW_OWNER).is_null() };
    !owned && !title(hwnd).is_empty()
}

pub fn is_hung(hwnd: isize) -> bool {
    unsafe { IsHungAppWindow(h(hwnd)) != 0 }
}

fn window_rect(hwnd: isize) -> Option<Rect> {
    let mut r: RECT = unsafe { mem::zeroed() };
    (unsafe { GetWindowRect(h(hwnd), &mut r) } != 0).then(|| rect(r))
}

/// A window's visible frame and the invisible resize border around it.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// What you see on screen, without the drop shadow.
    pub visible: Rect,
    /// The full window rectangle Win32 positions, shadow border included.
    pub outer: Rect,
}

pub fn frame(hwnd: isize) -> Option<Frame> {
    let outer = window_rect(hwnd)?;
    let mut r: RECT = unsafe { mem::zeroed() };
    let hr = unsafe {
        DwmGetWindowAttribute(h(hwnd), DWMWA_EXTENDED_FRAME_BOUNDS, ptr::addr_of_mut!(r).cast(), mem::size_of::<RECT>() as u32)
    };
    let visible = if hr == 0 { rect(r) } else { outer };
    Some(Frame { visible, outer })
}

#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    pub handle: isize,
    pub work: Rect,
    pub monitor: Rect,
    /// Display scale, 1.0 at 100%.
    pub scale: f64,
}

pub fn monitor(hwnd: isize) -> Option<Monitor> {
    unsafe {
        let hmon = MonitorFromWindow(h(hwnd), MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = mem::zeroed();
        info.cbSize = mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut info) == 0 {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        Some(Monitor {
            handle: hmon as isize,
            work: rect(info.rcWork),
            monitor: rect(info.rcMonitor),
            scale: dpi_x.max(96) as f64 / 96.0,
        })
    }
}

/// Bring a maximised or snapped window back to a normal, movable state
/// without stealing focus. Returns false if nothing needed doing.
pub fn unsnap(hwnd: isize) -> bool {
    if !is_maximized(hwnd) && !is_snapped(hwnd) {
        return false;
    }
    // Windows keeps snapped windows square and won't let them be resized
    // freely, so they have to be restored first.
    unsafe { ShowWindow(h(hwnd), SW_SHOWNOACTIVATE) };
    true
}

/// Move a window so its *visible* frame lands exactly on `target`. With
/// `wait` false the request is queued to the window's own thread and this
/// returns at once, so many windows can move in parallel.
pub fn place(hwnd: isize, target: Rect, frame: Frame, wait: bool) {
    let (v, o) = (frame.visible, frame.outer);
    let (left, top) = (v.x - o.x, v.y - o.y);
    let (right, bottom) = (o.right() - v.right(), o.bottom() - v.bottom());
    unsafe {
        SetWindowPos(
            h(hwnd),
            ptr::null_mut(),
            target.x - left,
            target.y - top,
            target.w + left + right,
            target.h + top + bottom,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | if wait { 0 } else { SWP_ASYNCWINDOWPOS },
        );
    }
}

fn set_dword(hwnd: isize, attr: u32, value: u32) -> bool {
    unsafe { DwmSetWindowAttribute(h(hwnd), attr, ptr::addr_of!(value).cast::<c_void>(), 4) == 0 }
}

pub fn set_corners(hwnd: isize, preference: u32) -> bool {
    set_dword(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, preference)
}

pub fn set_border(hwnd: isize, color: u32) -> bool {
    set_dword(hwnd, DWMWA_BORDER_COLOR, color)
}

/// The Windows accent colour as (r, g, b).
pub fn accent_color() -> Option<(u8, u8, u8)> {
    let (mut argb, mut opaque) = (0u32, 0);
    let ok = unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) } == 0;
    ok.then(|| ((argb >> 16) as u8, (argb >> 8) as u8, argb as u8))
}

/// Round a window by clipping it to a rounded region, for apps that ignore
/// the DWM corner preference. Returns the size it was clipped at.
pub fn clip(hwnd: isize, diameter: i32) -> Option<(i32, i32)> {
    let f = frame(hwnd)?;
    let (v, o) = (f.visible, f.outer);
    if v.w < diameter || v.h < diameter {
        return None;
    }
    // Region coordinates are relative to the window's top-left corner, and
    // GDI regions exclude their right and bottom edge, hence the +1.
    let (x, y) = (v.x - o.x, v.y - o.y);
    unsafe {
        let region = CreateRoundRectRgn(x, y, x + v.w + 1, y + v.h + 1, diameter, diameter);
        if region.is_null() {
            return None;
        }
        // On success Windows owns the region; we only free it on failure.
        if SetWindowRgn(h(hwnd), region, 1) == 0 {
            DeleteObject(region);
            return None;
        }
    }
    Some((v.w, v.h))
}

pub fn unclip(hwnd: isize) {
    unsafe { SetWindowRgn(h(hwnd), ptr::null_mut(), 1) };
}
