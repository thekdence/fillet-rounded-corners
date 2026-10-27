//! Dims every window except the one you're using, the way HazeOver does on
//! macOS.
//!
//! Each screen gets a black, click-through, semi-transparent "cover" kept in
//! the z-order just beneath the window that should stay bright, so
//! everything behind it is shaded. The taskbar is always on top, so it gets
//! a small cover of its own. The main cover spans the work area, not the
//! whole screen: a window exactly the size of a screen looks like a
//! full-screen app to Windows, which then drops that screen's taskbar out of
//! always-on-top. DWM composites all of these, so a still shade
//! costs nothing on the CPU.
//!
//! Changes fade rather than snap. When focus moves, the new window is bright
//! at once (Windows has already raised it) and the old one eases down under
//! a "ghost" cover shaped like it; then the screen's cover slips beneath the
//! new window and the ghost goes, in one atomic step so nothing flickers.
//! Fades are stepped by a short-lived thread at the display's refresh rate,
//! up to 60 a second: each opacity change is a fixed-cost trip through
//! Windows, and a gentle fade looks no different at more.

use std::{
    cell::Cell,
    mem, ptr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::{
        Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO},
        Gdi::{CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetStockObject, SetWindowRgn, BLACK_BRUSH, HBRUSH, NULLREGION, RGN_AND},
    },
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        BeginDeferWindowPos, CreateWindowExW, DefWindowProcW, DeferWindowPos, DestroyWindow, EndDeferWindowPos,
        GetWindow, GetWindowRect, GetWindowThreadProcessId, IsWindow, PostMessageW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos,
        ShowWindow, GW_HWNDNEXT, GW_HWNDPREV, HWND_TOP, HWND_TOPMOST, LWA_ALPHA, SWP_HIDEWINDOW, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    },
};

use super::win;
use crate::layout::Rect;

/// The shade coming and going.
const FADE: Duration = Duration::from_millis(260);
/// A window being left behind easing into the shade.
const HANDOFF: Duration = Duration::from_millis(220);
/// Focus often passes through the shell for a moment (clicking a taskbar
/// button, Alt+Tab), so wait this long before lifting the shade.
const LIFT_DELAY: Duration = Duration::from_millis(150);
/// Windows reports a focus change a moment before the app raises its
/// window, so keep each cover pinned beneath its window for this long after.
const SETTLE: Duration = Duration::from_millis(400);

/// The previous window's frame and corner diameter, for its ghost cover.
pub type Shape = (Rect, i32);

/// A screen and the window that should stay bright on it (None: lift).
pub struct Target {
    pub monitor: win::Monitor,
    pub anchor: Option<isize>,
}

pub struct Dimmer {
    shades: Vec<Shade>,
    amount: f64,
    ticker: Ticker,
}

impl Dimmer {
    pub fn new(engine: isize, percent: u32) -> Self {
        Self { shades: Vec::new(), amount: percent as f64 / 100.0, ticker: Ticker::new(engine) }
    }

    /// The windows each screen is keeping bright right now.
    pub fn anchors(&self) -> Vec<isize> {
        self.shades.iter().map(|s| s.below).filter(|&b| b != 0).collect()
    }

    pub fn update(&mut self, targets: Vec<Target>, shapes: &dyn Fn(isize) -> Option<Shape>) {
        // Screens that went away lose their shade.
        self.shades.retain(|s| targets.iter().any(|t| t.monitor.handle == s.handle));
        let bars = win::taskbars();
        for target in targets {
            let index = match self.shades.iter().position(|s| s.handle == target.monitor.handle) {
                Some(i) => i,
                None if target.anchor.is_none() => continue,
                None => match Shade::new(&target.monitor) {
                    Some(shade) => {
                        self.shades.push(shade);
                        self.shades.len() - 1
                    }
                    None => continue,
                },
            };
            let shade = &mut self.shades[index];
            let area = cover_area(&target.monitor);
            let resized = shade.area != area;
            shade.screen = target.monitor.monitor;
            shade.area = area;
            if resized && shade.below != 0 && !matches!(shade.anim, Anim::Handoff { .. }) {
                // The display changed under a shade that's showing.
                shade.cover_below(shade.below);
                shade.place_bars(&bars);
            }
            let old = shade.below;
            let shape = if old != 0 { shapes(old) } else { None };
            shade.retarget(target.anchor, shape, self.amount, &bars);
        }
        self.kick();
    }

    /// Put any cover that something slipped above back in place.
    pub fn fix_order(&mut self) {
        for shade in &mut self.shades {
            shade.fix_order();
        }
    }

    pub fn set_amount(&mut self, percent: u32) {
        self.amount = percent as f64 / 100.0;
        for shade in &mut self.shades {
            shade.settle(self.amount);
        }
    }

    /// Step every fade; called once per display refresh while any runs.
    pub fn tick(&mut self) {
        let mut busy = false;
        for shade in &mut self.shades {
            busy |= shade.tick(self.amount);
            match shade.watch {
                Some(until) if Instant::now() < until => {
                    shade.fix_order();
                    busy = true;
                }
                Some(_) => shade.watch = None,
                None => {}
            }
        }
        if !busy {
            self.ticker.stop();
        }
    }

    fn kick(&mut self) {
        if self.shades.iter().any(|s| !matches!(s.anim, Anim::Idle) || s.watch.is_some()) {
            self.ticker.start();
        }
    }
}

impl Drop for Dimmer {
    fn drop(&mut self) {
        self.ticker.stop();
    }
}

enum Anim {
    Idle,
    /// The whole shade fading between two levels, after `delay`.
    Level { from: f64, to: f64, start: Instant, delay: Duration },
    /// The previous window fading under the ghost; then the cover moves
    /// beneath `to`.
    Handoff { to: isize, start: Instant },
}

/// One screen's shade.
struct Shade {
    handle: isize,
    screen: Rect,
    /// What the main cover spans.
    area: Rect,
    cover: Overlay,
    ghost: Overlay,
    /// Each taskbar on this screen, and the cover over it.
    bars: Vec<(isize, Overlay)>,
    /// The window the cover sits beneath; 0 while hidden.
    below: isize,
    level: f64,
    anim: Anim,
    /// Keep checking the cover's place in the z-order until then.
    watch: Option<Instant>,
}

impl Shade {
    fn new(monitor: &win::Monitor) -> Option<Self> {
        Some(Self {
            handle: monitor.handle,
            screen: monitor.monitor,
            area: cover_area(monitor),
            cover: Overlay::new()?,
            ghost: Overlay::new()?,
            bars: Vec::new(),
            below: 0,
            level: 0.0,
            anim: Anim::Idle,
            watch: None,
        })
    }

    fn lifting(&self) -> bool {
        matches!(self.anim, Anim::Level { to, .. } if to == 0.0)
    }

    fn retarget(&mut self, anchor: Option<isize>, old: Option<Shape>, amount: f64, bars: &[isize]) {
        self.finish_handoff();
        self.watch = Some(Instant::now() + SETTLE);
        let Some(new) = anchor else {
            if self.below != 0 && !self.lifting() {
                self.fade_to(0.0, LIFT_DELAY);
            }
            return;
        };
        if self.below == 0 || self.lifting() {
            // Coming (back) from nothing: the taskbar may have been raised
            // above its cover in the meantime.
            self.cover_below(new);
            self.place_bars(bars);
            self.fade_to(amount, Duration::ZERO);
            return;
        }
        if self.below == new {
            return;
        }
        let prev = self.below;
        // Fade the old window out only once the shade is fully in.
        if let (Anim::Idle, Some(shape)) = (&self.anim, old) {
            if self.show_ghost(prev, shape) {
                self.anim = Anim::Handoff { to: new, start: Instant::now() };
                return;
            }
        }
        self.cover_below(new);
    }

    /// The cover belongs directly beneath its window. Apps raise their
    /// window a moment after taking focus, and windows can be shown above
    /// it, so it's checked rather than assumed.
    fn fix_order(&mut self) {
        if self.below == 0 || matches!(self.anim, Anim::Handoff { .. }) {
            return;
        }
        // Other screens' covers sit beneath the same window, so step past
        // any of ours (all made on this thread) before judging.
        let mut next = unsafe { GetWindow(self.below as HWND, GW_HWNDNEXT) };
        while !next.is_null() && next as isize != self.cover.0 && is_ours(next) {
            next = unsafe { GetWindow(next, GW_HWNDNEXT) };
        }
        if next as isize != self.cover.0 && unsafe { IsWindow(self.below as HWND) } != 0 {
            let s = self.area;
            unsafe {
                SetWindowPos(self.cover.0 as HWND, self.below as HWND, s.x, s.y, s.w, s.h, SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            }
        }
        // Windows re-asserts the taskbar's place from time to time.
        let slipped = self.bars.iter().any(|(bar, overlay)| {
            let mut above = unsafe { GetWindow(*bar as HWND, GW_HWNDPREV) };
            while !above.is_null() && above as isize != overlay.0 && is_ours(above) {
                above = unsafe { GetWindow(above, GW_HWNDPREV) };
            }
            above as isize != overlay.0
        });
        if slipped {
            let bars: Vec<isize> = self.bars.iter().map(|(bar, _)| *bar).collect();
            self.place_bars(&bars);
        }
    }

    fn show_ghost(&self, prev: isize, (frame, diameter): Shape) -> bool {
        unsafe {
            // Shaped like the window, and only the part on this screen.
            let rgn = CreateRoundRectRgn(0, 0, frame.w + 1, frame.h + 1, diameter, diameter);
            let s = self.screen;
            let clip = CreateRectRgn(s.x - frame.x, s.y - frame.y, s.right() - frame.x, s.bottom() - frame.y);
            let kind = CombineRgn(rgn, rgn, clip, RGN_AND);
            DeleteObject(clip);
            if kind == NULLREGION || kind == 0 {
                DeleteObject(rgn);
                return false;
            }
            // The window now owns the region.
            SetWindowRgn(self.ghost.0 as HWND, rgn, 0);
            self.ghost.alpha(0.0);
            let above = GetWindow(prev as HWND, GW_HWNDPREV);
            SetWindowPos(
                self.ghost.0 as HWND,
                if above.is_null() { HWND_TOP } else { above },
                frame.x,
                frame.y,
                frame.w,
                frame.h,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            );
        }
        true
    }

    fn finish_handoff(&mut self) {
        let Anim::Handoff { to, .. } = self.anim else { return };
        self.anim = Anim::Idle;
        if unsafe { IsWindow(to as HWND) } == 0 {
            self.ghost.hide();
            return;
        }
        // Both at once, or the old window would flash bright or doubly dark
        // for a frame.
        let s = self.area;
        unsafe {
            let mut dwp = BeginDeferWindowPos(2);
            dwp = DeferWindowPos(dwp, self.cover.0 as HWND, to as HWND, s.x, s.y, s.w, s.h, SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            dwp = DeferWindowPos(
                dwp,
                self.ghost.0 as HWND,
                ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if !dwp.is_null() {
                EndDeferWindowPos(dwp);
            }
        }
        self.below = to;
    }

    fn cover_below(&mut self, hwnd: isize) {
        let s = self.area;
        if self.below == 0 {
            self.cover.alpha(self.level);
        }
        unsafe {
            SetWindowPos(self.cover.0 as HWND, hwnd as HWND, s.x, s.y, s.w, s.h, SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW);
        }
        self.below = hwnd;
    }

    /// Lay a cover over each taskbar on this screen, just above it.
    fn place_bars(&mut self, all: &[isize]) {
        let mine: Vec<(isize, Rect)> = all
            .iter()
            .filter(|&&bar| win::monitor(bar).is_some_and(|m| m.handle == self.handle))
            .filter_map(|&bar| {
                let mut r = unsafe { mem::zeroed() };
                (unsafe { GetWindowRect(bar as HWND, &mut r) } != 0).then(|| (bar, Rect::from_edges(r.left, r.top, r.right, r.bottom)))
            })
            // An auto-hidden taskbar is tucked off the screen.
            .filter(|(_, r)| visible_part(r, &self.screen) > 4)
            .collect();
        self.bars.retain(|(bar, _)| mine.iter().any(|(b, _)| b == bar));
        for (bar, rect) in mine {
            if !self.bars.iter().any(|(b, _)| *b == bar) {
                let Some(overlay) = Overlay::new() else { continue };
                self.bars.push((bar, overlay));
            }
            let Some((_, overlay)) = self.bars.iter().find(|(b, _)| *b == bar) else { continue };
            overlay.alpha(self.level);
            unsafe {
                let above = GetWindow(bar as HWND, GW_HWNDPREV);
                let (after, keep) = if above as isize == overlay.0 {
                    (ptr::null_mut(), SWP_NOZORDER)
                } else if above.is_null() {
                    (HWND_TOPMOST, 0)
                } else {
                    (above, 0)
                };
                SetWindowPos(overlay.0 as HWND, after, rect.x, rect.y, rect.w, rect.h, SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW | keep);
            }
        }
    }

    fn fade_to(&mut self, to: f64, delay: Duration) {
        self.anim = Anim::Level { from: self.level, to, start: Instant::now(), delay };
    }

    fn set_level(&mut self, level: f64) {
        self.level = level;
        self.cover.alpha(level);
        for (_, bar) in &self.bars {
            bar.alpha(level);
        }
    }

    /// Jump to a new amount, e.g. while its slider is dragged.
    fn settle(&mut self, amount: f64) {
        // A lifting shade keeps fading out.
        if self.lifting() {
            return;
        }
        match self.anim {
            Anim::Level { .. } => {
                self.anim = Anim::Idle;
                self.set_level(amount);
            }
            _ if self.below != 0 => self.set_level(amount),
            _ => {}
        }
    }

    /// Returns whether this shade is still animating.
    fn tick(&mut self, amount: f64) -> bool {
        match self.anim {
            Anim::Idle => false,
            Anim::Level { from, to, start, delay } => {
                let Some(t) = start.elapsed().checked_sub(delay).map(|e| progress(e, FADE)) else { return true };
                self.set_level(from + (to - from) * ease(t));
                if t < 1.0 {
                    return true;
                }
                self.anim = Anim::Idle;
                if to == 0.0 {
                    self.cover.hide();
                    for (_, bar) in &self.bars {
                        bar.hide();
                    }
                    self.below = 0;
                }
                false
            }
            Anim::Handoff { start, .. } => {
                let t = progress(start.elapsed(), HANDOFF);
                self.ghost.alpha(amount * ease(t));
                if t < 1.0 {
                    return true;
                }
                self.finish_handoff();
                false
            }
        }
    }
}

fn progress(elapsed: Duration, total: Duration) -> f64 {
    (elapsed.as_secs_f64() / total.as_secs_f64()).min(1.0)
}

/// Gentle at both ends, like macOS's default curve.
fn ease(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// One of the dimmer's own windows, which are all made on the engine thread.
fn is_ours(hwnd: HWND) -> bool {
    unsafe { GetWindowThreadProcessId(hwnd, ptr::null_mut()) == GetCurrentThreadId() }
}

fn cover_area(monitor: &win::Monitor) -> Rect {
    let (work, mut area) = (monitor.work, monitor.monitor);
    if work != area {
        return work;
    }
    // No taskbar in the way (it's auto-hidden): stop a pixel short of full
    // screen.
    area.h -= 1;
    area
}

fn visible_part(r: &Rect, screen: &Rect) -> i32 {
    let w = r.right().min(screen.right()) - r.x.max(screen.x);
    let h = r.bottom().min(screen.bottom()) - r.y.max(screen.y);
    w.min(h)
}

/// A black, click-through window that never takes focus or shows in the
/// taskbar or Alt+Tab.
struct Overlay(isize, Cell<Option<u8>>);

impl Overlay {
    fn new() -> Option<Self> {
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
            (!hwnd.is_null()).then(|| Self(hwnd as isize, Cell::new(None)))
        }
    }

    fn alpha(&self, level: f64) {
        let alpha = (level.clamp(0.0, 1.0) * 255.0).round() as u8;
        if self.1.replace(Some(alpha)) != Some(alpha) {
            unsafe { SetLayeredWindowAttributes(self.0 as HWND, 0, alpha, LWA_ALPHA) };
        }
    }

    fn hide(&self) {
        unsafe { ShowWindow(self.0 as HWND, SW_HIDE) };
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0 as HWND) };
    }
}

/// Posts `WM_FADE_TICK` to the engine once a frame while a fade runs. The
/// thread only exists for the length of a fade.
struct Ticker {
    engine: isize,
    /// Bumped to stop the running thread; a thread exits once it's stale.
    generation: Arc<AtomicU64>,
    running: Option<u64>,
}

impl Ticker {
    fn new(engine: isize) -> Self {
        Self { engine, generation: Arc::new(AtomicU64::new(0)), running: None }
    }

    fn start(&mut self) {
        if self.running.is_some() {
            return;
        }
        let gen = self.generation.load(Ordering::SeqCst);
        let (current, engine) = (self.generation.clone(), self.engine);
        let frame = refresh_period();
        let spawned = thread::Builder::new().name("fillet-fade".into()).spawn(move || {
            // Paced off the clock rather than DwmFlush, which waits for a
            // frame that a still desktop never draws.
            let mut next = Instant::now();
            while current.load(Ordering::SeqCst) == gen {
                unsafe { PostMessageW(engine as HWND, super::WM_FADE_TICK, 0, 0) };
                next += frame;
                match next.checked_duration_since(Instant::now()) {
                    Some(wait) => thread::sleep(wait),
                    None => next = Instant::now(),
                }
            }
        });
        if spawned.is_ok() {
            self.running = Some(gen);
        }
    }

    fn stop(&mut self) {
        if self.running.take().is_some() {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// How often to step a fade: once per display frame, but no more than 60
/// times a second.
fn refresh_period() -> Duration {
    const MIN: Duration = Duration::from_micros(16_667);
    let mut info: DWM_TIMING_INFO = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<DWM_TIMING_INFO>() as u32;
    let ok = unsafe { DwmGetCompositionTimingInfo(ptr::null_mut(), &mut info) } == 0;
    let hz = info.rateRefresh.uiNumerator as f64 / info.rateRefresh.uiDenominator.max(1) as f64;
    if ok && (30.0..=500.0).contains(&hz) {
        Duration::from_secs_f64(1.0 / hz).max(MIN)
    } else {
        MIN
    }
}

