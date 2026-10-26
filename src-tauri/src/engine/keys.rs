//! Fallback for shortcuts another app (or Windows itself) has already
//! registered, such as Windows 11's own Win+Alt+Arrow snapping.
//!
//! A low-level keyboard hook sees every key press on the system, so it runs
//! on a thread of its own that does nothing else: a busy engine thread can
//! never delay anyone's typing. It only exists while there are shortcuts
//! that need it.

use std::{
    cell::RefCell,
    mem, ptr,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, PostMessageW, PostThreadMessageW, SetWindowsHookExW,
            UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN,
            WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

const MOD_ALT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_SHIFT: u32 = 4;
const MOD_WIN: u32 = 8;

/// An unassigned virtual key. Tapping it after a swallowed Win or Alt combo
/// stops Windows from treating the modifier as pressed on its own, which
/// would open the Start menu or an app's menu bar.
const VK_MASK: u16 = 0xE8;
/// Tags our own synthetic key presses so the hook can skip them.
const MASK_TAG: usize = 0x4649_4C54; // "FILT"

/// A shortcut to catch: modifiers, key, and the `WM_HOTKEY` id to post.
#[derive(Clone, Copy)]
pub struct Binding {
    pub mods: u32,
    pub vk: u32,
    pub id: usize,
}

struct Hook {
    target: isize,
    bindings: Vec<Binding>,
    /// The key of a combo we're holding down, so its repeats and release
    /// are swallowed too.
    held: Option<u32>,
}

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

pub struct KeyHook {
    thread_id: u32,
    thread: JoinHandle<()>,
}

impl KeyHook {
    /// Catch `bindings` and post them to `target` as `WM_HOTKEY`.
    pub fn start(target: isize, bindings: Vec<Binding>) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("fillet-keys".into())
            .spawn(move || unsafe {
                HOOK.with(|h| *h.borrow_mut() = Some(Hook { target, bindings, held: None }));
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), GetModuleHandleW(ptr::null()), 0);
                let _ = tx.send((!hook.is_null()).then(|| GetCurrentThreadId()));
                if hook.is_null() {
                    return;
                }
                let mut msg: MSG = mem::zeroed();
                while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                    DispatchMessageW(&msg);
                }
                UnhookWindowsHookEx(hook);
            })
            .ok()?;
        match rx.recv().ok().flatten() {
            Some(thread_id) => Some(Self { thread_id, thread }),
            None => {
                let _ = thread.join();
                None
            }
        }
    }

    pub fn stop(self) {
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        let _ = self.thread.join();
    }
}

fn is_down(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) < 0 }
}

fn held_modifiers() -> u32 {
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;
    let mut mods = 0;
    if is_down(VK_MENU) {
        mods |= MOD_ALT;
    }
    if is_down(VK_CONTROL) {
        mods |= MOD_CTRL;
    }
    if is_down(VK_SHIFT) {
        mods |= MOD_SHIFT;
    }
    if is_down(VK_LWIN) || is_down(VK_RWIN) {
        mods |= MOD_WIN;
    }
    mods
}

fn tap_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MASK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: MASK_TAG } },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), mem::size_of::<INPUT>() as i32) };
}

unsafe extern "system" fn on_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // Synthetic input (macro pads, scripts) counts, but not our own mask
        // key.
        if key.dwExtraInfo != MASK_TAG && HOOK.with(|h| handle(&mut h.borrow_mut(), wparam as u32, key.vkCode)) {
            return 1;
        }
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// Returns true to swallow the key.
fn handle(hook: &mut Option<Hook>, msg: u32, vk: u32) -> bool {
    let Some(hook) = hook.as_mut() else { return false };
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let mods = held_modifiers();
            let Some(binding) = hook.bindings.iter().find(|b| b.vk == vk && b.mods == mods) else {
                // Not a shortcut (any more): never leave a key stuck as held.
                if hook.held == Some(vk) {
                    hook.held = None;
                }
                return false;
            };
            if hook.held == Some(vk) {
                return true; // auto-repeat
            }
            hook.held = Some(vk);
            unsafe { PostMessageW(hook.target as HWND, WM_HOTKEY, binding.id, 0) };
            if mods & (MOD_WIN | MOD_ALT) != 0 {
                tap_mask_key();
            }
            true
        }
        WM_KEYUP | WM_SYSKEYUP if hook.held == Some(vk) => {
            hook.held = None;
            true
        }
        _ => false,
    }
}
