//! Dims every window except the active one.
//!
//! One black, click-through, semi-transparent window covers every screen and
//! is kept in the z-order just beneath the active window, so everything
//! behind it (other windows and the wallpaper) is shaded and the active
//! window, the taskbar and always-on-top windows aren't. DWM composites it,
//! so it costs nothing on the CPU.

use std::{mem, ptr};

use windows_sys::Win32::{
    Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, RegisterClassW, SetLayeredWindowAttributes,
        SetWindowPos, ShowWindow, LWA_ALPHA, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_SHOWWINDOW, SW_HIDE, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    },
};

pub struct Dimmer {
    hwnd: isize,
}

impl Dimmer {
    pub fn new(percent: u32) -> Option<Self> {
        unsafe {
            let class: Vec<u16> = "FilletDim\0".encode_utf16().collect();
            let instance = GetModuleHandleW(ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
                ..mem::zeroed()
            };
            // Fails harmlessly once the class exists.
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                ptr::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            let dimmer = Self { hwnd: hwnd as isize };
            dimmer.set_amount(percent);
            Some(dimmer)
        }
    }

    pub fn set_amount(&self, percent: u32) {
        let alpha = (percent.min(100) as f64 * 2.55).round() as u8;
        unsafe { SetLayeredWindowAttributes(self.hwnd as _, 0, alpha, LWA_ALPHA) };
    }

    /// Shade everything beneath `hwnd`. Re-measures the screens each time,
    /// so a monitor being added or removed is picked up on the next focus
    /// change.
    pub fn show_below(&self, hwnd: isize) {
        unsafe {
            SetWindowPos(
                self.hwnd as _,
                hwnd as _,
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            );
        }
    }

    pub fn hide(&self) {
        unsafe { ShowWindow(self.hwnd as _, SW_HIDE) };
    }
}

impl Drop for Dimmer {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd as _) };
    }
}
