//! The always-on part of Fillet: one thread that listens for window events
//! and global shortcuts, then rounds and arranges windows.
//!
//! Nothing polls. Windows tells us when a window appears, gains focus, or
//! finishes moving; we batch those for a moment and handle each window once.
//! Between events the thread sleeps in `GetMessageW` and costs nothing.

mod dim;
mod keys;
mod win;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    mem, ptr,
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
        Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetAncestor, GetMessageW, KillTimer,
            PostMessageW, PostQuitMessage, RegisterClassW, SetTimer, GA_ROOT, HWND_MESSAGE, MSG, WM_APP, WM_HOTKEY,
            WM_TIMER, WNDCLASSW,
        },
    },
};

use crate::{
    config::{self, Action, Border, Config, CornerStyle, Highlight},
    layout::{Layout, Rect},
};

const WM_RELOAD: u32 = WM_APP + 1;
const WM_ACTION: u32 = WM_APP + 2;
const WM_SUSPEND_HOTKEYS: u32 = WM_APP + 3;
const WM_SHUTDOWN: u32 = WM_APP + 4;
/// From the dimmer's fade thread, once per display refresh.
const WM_FADE_TICK: u32 = WM_APP + 5;

const FLUSH_TIMER: usize = 1;
const SETTLE_TIMER: usize = 2;
/// Once a spacing slider has rested this long, windows get their final,
/// pixel-exact placement.
const SETTLE_DELAY_MS: u32 = 220;
/// How long a window may take over one live move before it's sent the next
/// regardless (it may simply be unable to reach that size).
const LIVE_CATCH_UP: Duration = Duration::from_millis(120);
/// Long enough to fold a burst of events (show, focus, resize) into one pass,
/// short enough that it feels instant.
const FLUSH_DELAY_MS: u32 = 90;
/// A window that never stops moving must not hold everything else up, so
/// stop pushing the flush back once events have waited this long.
const FLUSH_MAX_WAIT: Duration = Duration::from_millis(400);

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MOVESIZESTART: u32 = 0x000A;
const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

/// How close, in logical pixels, a window must be to a snap zone for
/// automatic tidying to treat it as snapped.
const SNAP_TOLERANCE: f64 = 12.0;
/// A window maximised this soon after a drag ends was dropped on the top
/// edge of the screen.
const DRAG_TO_TOP: Duration = Duration::from_millis(1500);

/// Called on the engine thread with the actions whose shortcut another app
/// (or Windows) had already registered, and which Fillet now takes over.
pub type ConflictFn = Box<dyn Fn(Vec<Action>) + Send>;

/// Handle to the engine thread. Cheap to share; every method just posts a
/// message to the thread.
pub struct Engine {
    hwnd: isize,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Engine {
    pub fn start(config: Arc<Mutex<Config>>, on_conflicts: ConflictFn) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("fillet-engine".into())
            .spawn(move || run(config, on_conflicts, tx))
            .expect("failed to start the window engine");
        let hwnd = rx.recv().expect("window engine failed to start");
        Self { hwnd, thread: Mutex::new(Some(thread)) }
    }

    fn post(&self, msg: u32, wparam: usize) {
        unsafe { PostMessageW(self.hwnd as HWND, msg, wparam, 0) };
    }

    /// Pick up a changed config.
    pub fn reload(&self) {
        self.post(WM_RELOAD, 0);
    }

    pub fn run(&self, action: Action) {
        let index = Action::ALL.iter().position(|a| *a == action).unwrap_or(0);
        self.post(WM_ACTION, index);
    }

    /// Release the shortcuts while the settings window records a new one,
    /// otherwise pressing an existing shortcut would trigger it instead.
    pub fn suspend_hotkeys(&self, suspend: bool) {
        self.post(WM_SUSPEND_HOTKEYS, suspend as usize);
    }

    /// Put every window back the way Windows draws it, then stop.
    pub fn shutdown(&self) {
        let Some(thread) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        self.post(WM_SHUTDOWN, 0);
        let _ = thread.join();
    }
}

// ---------------------------------------------------------------------------
// Engine thread

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Windows with events waiting, and whether the event could mean the
    /// window was snapped (and so might need tidying). Kept apart from
    /// `STATE` so the event hook never touches the main state.
    static PENDING: RefCell<HashMap<isize, bool>> = RefCell::new(HashMap::new());
    /// Windows the user is dragging or resizing right now.
    static MOVING: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
    /// When each window last finished being dragged, to tell Windows'
    /// drag-to-top maximize apart from the maximize button.
    static DRAGGED: RefCell<HashMap<isize, Instant>> = RefCell::new(HashMap::new());
    /// When the oldest pending event arrived.
    static PENDING_SINCE: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The foreground window, as last reported by Windows.
    static ACTIVE: Cell<isize> = const { Cell::new(0) };
    static ENGINE_HWND: Cell<isize> = const { Cell::new(0) };
}

struct State {
    hwnd: isize,
    shared: Arc<Mutex<Config>>,
    cfg: Config,
    on_conflicts: ConflictFn,
    hooks: Vec<HWINEVENTHOOK>,
    location_hook: Option<HWINEVENTHOOK>,
    hotkeys: Vec<i32>,
    /// Catches the shortcuts `RegisterHotKey` couldn't claim.
    key_hook: Option<keys::KeyHook>,
    hotkeys_suspended: bool,
    /// Windows whose DWM corner or border setting we changed.
    styled: HashSet<isize>,
    /// Windows we clipped to a rounded region, and the size we clipped at.
    clipped: HashMap<isize, (i32, i32)>,
    /// The window wearing the active-window border, if any.
    highlighted: isize,
    /// Shades the windows behind the active one, while that's turned on.
    dimmer: Option<dim::Dimmer>,
    /// What each screen's shade was last told to keep bright.
    dim_key: Vec<(isize, Rect, Option<isize>)>,
    /// Where automatic tidying last put each window and when, so a window
    /// that refuses a size can't send us into a loop.
    placed: HashMap<isize, (Rect, Instant)>,
    /// Windows following a spacing slider as it's dragged: their layout,
    /// and the last move we asked of them.
    live: HashMap<isize, (Layout, Option<(Rect, Instant)>)>,
    names: HashMap<u32, Option<String>>,
}

fn run(shared: Arc<Mutex<Config>>, on_conflicts: ConflictFn, ready: mpsc::Sender<isize>) {
    unsafe {
        // Work in physical pixels on every monitor.
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let class: Vec<u16> = "FilletEngine\0".encode_utf16().collect();
        let instance = GetModuleHandleW(ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..mem::zeroed()
        };
        RegisterClassW(&wc);
        // A message-only window: invisible, and the target for timers,
        // shortcuts and messages from the UI.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            instance,
            ptr::null(),
        ) as isize;
        ENGINE_HWND.set(hwnd);

        let cfg = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut state = State {
            hwnd,
            shared,
            cfg: cfg.clone(),
            on_conflicts,
            hooks: Vec::new(),
            location_hook: None,
            hotkeys: Vec::new(),
            key_hook: None,
            hotkeys_suspended: false,
            styled: HashSet::new(),
            clipped: HashMap::new(),
            highlighted: 0,
            dimmer: None,
            dim_key: Vec::new(),
            placed: HashMap::new(),
            live: HashMap::new(),
            names: HashMap::new(),
        };
        state.activate();
        STATE.with(|s| *s.borrow_mut() = Some(state));
        let _ = ready.send(hwnd);

        let mut msg: MSG = mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = match msg {
        WM_TIMER if wparam == FLUSH_TIMER => {
            unsafe { KillTimer(hwnd, FLUSH_TIMER) };
            with_state(State::flush)
        }
        WM_TIMER if wparam == SETTLE_TIMER => {
            unsafe { KillTimer(hwnd, SETTLE_TIMER) };
            with_state(State::settle)
        }
        WM_HOTKEY => {
            let action = Action::ALL.get(wparam.wrapping_sub(1)).copied();
            with_state(|s| {
                if let Some(a) = action {
                    s.run(a);
                }
            })
        }
        WM_ACTION => {
            let action = Action::ALL.get(wparam).copied();
            with_state(|s| {
                if let Some(a) = action {
                    s.run(a);
                }
            })
        }
        WM_FADE_TICK => with_state(State::fade_tick),
        WM_RELOAD => with_state(State::reload),
        WM_SUSPEND_HOTKEYS => with_state(|s| s.suspend_hotkeys(wparam != 0)),
        WM_SHUTDOWN => {
            with_state(State::deactivate);
            unsafe {
                DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            true
        }
        _ => false,
    };
    if handled {
        0
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

fn with_state(f: impl FnOnce(&mut State)) -> bool {
    STATE.with(|s| {
        // A Win32 call can, rarely, pump messages back into us. Skip rather
        // than re-enter; the next event will catch up.
        if let Ok(mut state) = s.try_borrow_mut() {
            if let Some(state) = state.as_mut() {
                f(state);
            }
        }
    });
    true
}

/// The WinEvent hook. Runs on the engine thread between messages, so it
/// only records what happened and schedules a flush.
unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    thread: u32,
    _time: u32,
) {
    // OBJID_WINDOW / CHILDID_SELF: the window itself, not a caret, cursor or
    // control inside it. Windows this thread made are the dimmer's own.
    if hwnd.is_null() || id_object != 0 || id_child != 0 || thread == unsafe { GetCurrentThreadId() } {
        return;
    }
    let id = hwnd as isize;
    let may_tidy = match event {
        EVENT_SYSTEM_MOVESIZESTART => {
            MOVING.with(|m| m.borrow_mut().insert(id));
            return;
        }
        EVENT_SYSTEM_MOVESIZEEND => {
            MOVING.with(|m| m.borrow_mut().remove(&id));
            DRAGGED.with(|d| d.borrow_mut().insert(id, Instant::now()));
            true
        }
        EVENT_OBJECT_LOCATIONCHANGE => {
            // Child windows move all the time; only top-level ones matter,
            // and not while the user is still dragging.
            if unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd || MOVING.with(|m| m.borrow().contains(&id)) {
                return;
            }
            true
        }
        EVENT_SYSTEM_FOREGROUND => {
            // Move the highlight now rather than after the flush delay, so
            // it keeps up with clicking around.
            ACTIVE.set(unsafe { GetAncestor(hwnd, GA_ROOT) } as isize);
            with_state(State::sync_focus);
            false
        }
        EVENT_OBJECT_SHOW | EVENT_SYSTEM_MINIMIZEEND => true,
        _ => false,
    };
    PENDING.with(|p| *p.borrow_mut().entry(id).or_default() |= may_tidy);
    // Re-arming resets the countdown, so a burst of events flushes once.
    let since = PENDING_SINCE.get().unwrap_or_else(Instant::now);
    PENDING_SINCE.set(Some(since));
    if since.elapsed() < FLUSH_MAX_WAIT {
        unsafe { SetTimer(ENGINE_HWND.get() as HWND, FLUSH_TIMER, FLUSH_DELAY_MS, None) };
    }
}

fn hook(event: u32) -> Option<HWINEVENTHOOK> {
    // Focus moving to Fillet's own settings window still matters: the
    // highlight leaves the window that had it, and the shade moves beneath
    // the settings. The settings window takes focus before it's shown, so
    // its showing up counts too.
    let own = matches!(event, EVENT_SYSTEM_FOREGROUND | EVENT_OBJECT_SHOW);
    let skip_own = if own { 0 } else { WINEVENT_SKIPOWNPROCESS };
    let hook = unsafe {
        SetWinEventHook(event, event, ptr::null_mut(), Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT | skip_own)
    };
    (!hook.is_null()).then_some(hook)
}

impl State {
    // -- lifecycle ----------------------------------------------------------

    /// Start listening and style every open window.
    fn activate(&mut self) {
        if !self.cfg.enabled {
            return;
        }
        if self.hooks.is_empty() {
            self.hooks = [
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_MOVESIZESTART,
                EVENT_SYSTEM_MOVESIZEEND,
                EVENT_SYSTEM_MINIMIZEEND,
                EVENT_OBJECT_SHOW,
            ]
            .into_iter()
            .filter_map(hook)
            .collect();
        }
        self.update_location_hook();
        self.register_hotkeys();
        ACTIVE.set(win::foreground());
        self.style_all();
    }

    /// Stop listening and undo everything we did to other windows.
    fn deactivate(&mut self) {
        for hook in self.hooks.drain(..).chain(self.location_hook.take()) {
            unsafe { UnhookWinEvent(hook) };
        }
        self.unregister_hotkeys();
        self.unstyle_all();
        self.dimmer = None;
        self.dim_key.clear();
        self.live.clear();
        PENDING.with(|p| p.borrow_mut().clear());
        PENDING_SINCE.set(None);
    }

    fn reload(&mut self) {
        let next = self.shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let prev = mem::replace(&mut self.cfg, next.clone());
        if prev == next {
            return;
        }
        if !next.enabled {
            self.deactivate();
            return;
        }
        if !prev.enabled {
            self.activate();
            return;
        }

        if prev.hotkeys != next.hotkeys {
            self.register_hotkeys();
        }
        self.update_location_hook();

        let restyle = prev.round_corners != next.round_corners
            || prev.corner_style != next.corner_style
            || prev.border != next.border
            || prev.border_color != next.border_color
            || prev.highlight != next.highlight
            || prev.highlight_color != next.highlight_color
            || prev.clip_radius != next.clip_radius
            || prev.clip_apps != next.clip_apps
            || prev.ignored_apps != next.ignored_apps;
        if restyle {
            self.unstyle_all();
            self.style_all();
        }

        if prev.dim_amount != next.dim_amount {
            if let Some(dimmer) = &mut self.dimmer {
                dimmer.set_amount(next.dim_amount);
            }
        }
        if prev.dim_inactive != next.dim_inactive || prev.dim_each_screen != next.dim_each_screen {
            self.sync_focus();
        }

        if prev.margin != next.margin || prev.gap != next.gap {
            self.respace(&prev);
        }
    }

    /// Windows sitting in a layout with the old spacing follow the new one,
    /// so dragging a slider moves the desktop with it.
    fn respace(&mut self, prev: &Config) {
        // Windows already following the drag may still be mid-move, so they
        // keep their layout instead of being measured again.
        for hwnd in win::top_level_windows() {
            if self.live.contains_key(&hwnd) || !self.can_arrange(hwnd) || win::is_maximized(hwnd) {
                continue;
            }
            let (Some(frame), Some(mon)) = (win::frame(hwnd), win::monitor(hwnd)) else { continue };
            let (m, g) = spacing(prev, mon.scale);
            if let Some(layout) = Layout::current(frame.visible, mon.work, m, g) {
                self.live.insert(hwnd, (layout, None));
            }
        }
        // Ask every window to move at once rather than waiting on each app
        // in turn; that's what keeps a drag smooth.
        let hwnds: Vec<isize> = self.live.keys().copied().collect();
        for hwnd in hwnds {
            let (Some(frame), Some(mon)) = (win::frame(hwnd), win::monitor(hwnd)) else {
                self.live.remove(&hwnd);
                continue;
            };
            let Some((layout, sent)) = self.live.get_mut(&hwnd) else { continue };
            // An app still working through its last move gets skipped this
            // tick, so slow apps never build up a backlog of stale sizes.
            if let Some((last, at)) = sent {
                if !frame.visible.near(last, 1) && at.elapsed() < LIVE_CATCH_UP {
                    continue;
                }
            }
            let (m, g) = spacing(&self.cfg, mon.scale);
            let target = layout.bounds(mon.work, m, g);
            win::place(hwnd, target, frame, false);
            *sent = Some((target, Instant::now()));
        }
        unsafe { SetTimer(self.hwnd as HWND, SETTLE_TIMER, SETTLE_DELAY_MS, None) };
    }

    /// The slider has come to rest: place each window exactly.
    fn settle(&mut self) {
        for (hwnd, (layout, _)) in mem::take(&mut self.live) {
            if win::exists(hwnd) && self.can_arrange(hwnd) {
                self.put(hwnd, layout);
            }
        }
    }

    // -- shortcuts ----------------------------------------------------------

    fn register_hotkeys(&mut self) {
        self.unregister_hotkeys();
        if self.hotkeys_suspended || !self.cfg.enabled {
            return;
        }
        let mut taken = Vec::new();
        let mut fallback = Vec::new();
        for (i, action) in Action::ALL.into_iter().enumerate() {
            let Some(key) = self.cfg.hotkeys.get(action) else { continue };
            let id = i as i32 + 1;
            if unsafe { RegisterHotKey(self.hwnd as HWND, id, key.mods | MOD_NOREPEAT, key.vk) } != 0 {
                self.hotkeys.push(id);
            } else {
                // Already registered elsewhere, so catch it before it gets
                // there instead.
                taken.push(action);
                fallback.push(keys::Binding { mods: key.mods, vk: key.vk, id: id as usize });
            }
        }
        if !fallback.is_empty() {
            self.key_hook = keys::KeyHook::start(self.hwnd, fallback);
        }
        (self.on_conflicts)(taken);
    }

    fn unregister_hotkeys(&mut self) {
        for id in self.hotkeys.drain(..) {
            unsafe { UnregisterHotKey(self.hwnd as HWND, id) };
        }
        if let Some(hook) = self.key_hook.take() {
            hook.stop();
        }
    }

    fn suspend_hotkeys(&mut self, suspend: bool) {
        self.hotkeys_suspended = suspend;
        self.register_hotkeys();
    }

    /// Location changes are by far the busiest event, so only listen for
    /// them when a feature needs them.
    fn update_location_hook(&mut self) {
        let c = &self.cfg;
        let needed = c.enabled && (c.auto_tidy || c.pad_maximized || (c.round_corners && !c.clip_apps.is_empty()));
        match (needed, self.location_hook) {
            (true, None) => self.location_hook = hook(EVENT_OBJECT_LOCATIONCHANGE),
            (false, Some(h)) => {
                unsafe { UnhookWinEvent(h) };
                self.location_hook = None;
            }
            _ => {}
        }
    }

    // -- per-window rules ---------------------------------------------------

    fn app_name(&mut self, hwnd: isize) -> Option<String> {
        let pid = win::process_id(hwnd);
        if self.names.len() > 512 {
            self.names.clear();
        }
        self.names.entry(pid).or_insert_with(|| win::process_name(pid)).clone()
    }

    fn is_ignored(&mut self, hwnd: isize) -> bool {
        let name = self.app_name(hwnd);
        name.is_some_and(|n| self.cfg.ignored_apps.contains(&n))
    }

    fn can_arrange(&mut self, hwnd: isize) -> bool {
        win::wants_layout(hwnd) && !win::is_hung(hwnd) && !self.is_ignored(hwnd)
    }

    // -- corners ------------------------------------------------------------

    fn style_all(&mut self) {
        for hwnd in win::top_level_windows() {
            self.style(hwnd);
        }
        self.sync_focus();
    }

    /// Apply the corner and border settings to one window.
    fn style(&mut self, hwnd: isize) {
        let eligible = win::wants_corners(hwnd) && !self.is_ignored(hwnd);
        if !eligible {
            // Maximised or gone full screen: a clip region would cut into
            // the screen edges, so take it off.
            if self.clipped.remove(&hwnd).is_some() && win::exists(hwnd) {
                win::unclip(hwnd);
            }
            return;
        }
        let Some(mon) = win::monitor(hwnd) else { return };

        if self.cfg.round_corners {
            let clip = self.app_name(hwnd).is_some_and(|n| self.cfg.clip_apps.contains(&n));
            if clip {
                let diameter = (self.cfg.clip_radius as f64 * 2.0 * mon.scale).round() as i32;
                let size = win::frame(hwnd).map(|f| (f.visible.w, f.visible.h));
                // Re-clipping repaints the window, so only do it on a resize.
                if self.clipped.get(&hwnd).copied() != size && !win::is_hung(hwnd) {
                    match win::clip(hwnd, diameter) {
                        Some(size) => self.clipped.insert(hwnd, size),
                        None => self.clipped.remove(&hwnd),
                    };
                }
            } else {
                let preference = match self.cfg.corner_style {
                    CornerStyle::Round => win::CORNER_ROUND,
                    CornerStyle::Small => win::CORNER_ROUND_SMALL,
                };
                if win::set_corners(hwnd, preference) {
                    self.styled.insert(hwnd);
                }
            }
        }

        self.paint_border(hwnd);
    }

    fn paint_border(&mut self, hwnd: isize) {
        let border = if hwnd == self.highlighted {
            Some(self.highlight_color())
        } else {
            match self.cfg.border {
                // Only needed to take a highlight back off.
                Border::System => (self.cfg.highlight != Highlight::Off).then_some(win::COLOR_DEFAULT),
                Border::None => Some(win::COLOR_NONE),
                Border::Custom => Some(self.cfg.border_colorref()),
            }
        };
        if let Some(color) = border {
            if win::set_border(hwnd, color) {
                self.styled.insert(hwnd);
            }
        }
    }

    fn highlight_color(&self) -> u32 {
        match self.cfg.highlight {
            Highlight::Custom => self.cfg.highlight_colorref(),
            _ => config::colorref(win::accent_color().unwrap_or((0x5b, 0x6c, 0xf0))),
        }
    }

    /// Follow the focus: move the active-window border, and the shade, to
    /// whichever window has it now.
    fn sync_focus(&mut self) {
        self.sync_dim();
        let active = ACTIVE.get();
        let want = if self.cfg.enabled
            && self.cfg.highlight != Highlight::Off
            && active != 0
            && win::wants_corners(active)
            && !self.is_ignored(active)
        {
            active
        } else {
            0
        };
        let prev = mem::replace(&mut self.highlighted, want);
        if prev == want {
            return;
        }
        if prev != 0 && self.styled.contains(&prev) && win::exists(prev) {
            self.paint_border(prev);
        }
        if want != 0 {
            self.paint_border(want);
        }
    }

    fn sync_dim(&mut self) {
        let on = self.cfg.enabled && self.cfg.dim_inactive;
        if !on && self.dimmer.is_none() {
            return;
        }
        let active = if on { win::dim_anchor(ACTIVE.get()) } else { None };
        let active_screen = active.and_then(win::monitor).map(|m| m.handle);
        let each = self.cfg.dim_each_screen && active.is_some();
        let monitors = win::monitors();
        let fronts = if each { front_windows(monitors.len()) } else { HashMap::new() };
        let targets: Vec<dim::Target> = monitors
            .into_iter()
            .map(|monitor| {
                let anchor = if !each || Some(monitor.handle) == active_screen {
                    active
                } else {
                    fronts.get(&monitor.handle).copied()
                };
                dim::Target { monitor, anchor }
            })
            .collect();
        let key: Vec<_> = targets.iter().map(|t| (t.monitor.handle, t.monitor.work, t.anchor)).collect();
        if key == self.dim_key {
            if let Some(dimmer) = &mut self.dimmer {
                dimmer.fix_order();
            }
            return;
        }
        self.dim_key = key;

        // Where the windows being left behind are, to fade them out.
        let leaving = self.dimmer.as_ref().map(|d| d.anchors()).unwrap_or_default();
        let shapes: HashMap<isize, dim::Shape> =
            leaving.into_iter().filter_map(|hwnd| Some((hwnd, self.window_shape(hwnd)?))).collect();
        let (engine, amount) = (self.hwnd, self.cfg.dim_amount);
        let dimmer = self.dimmer.get_or_insert_with(|| dim::Dimmer::new(engine, amount));
        dimmer.update(targets, &|hwnd| shapes.get(&hwnd).copied());
    }

    /// A window's visible frame and the diameter of its rounded corners.
    fn window_shape(&self, hwnd: isize) -> Option<dim::Shape> {
        if !win::exists(hwnd) {
            return None;
        }
        let frame = win::frame(hwnd)?.visible;
        let scale = win::monitor(hwnd)?.scale;
        let radius = if win::is_maximized(hwnd) {
            0
        } else if self.clipped.contains_key(&hwnd) {
            self.cfg.clip_radius
        } else if self.cfg.round_corners && self.cfg.corner_style == CornerStyle::Small {
            4
        } else {
            // Windows 11's own rounding, and Fillet's "Rounded".
            8
        };
        Some((frame, (radius as f64 * 2.0 * scale).round() as i32))
    }

    fn fade_tick(&mut self) {
        if let Some(dimmer) = &mut self.dimmer {
            dimmer.tick();
        }
    }

    fn unstyle_all(&mut self) {
        self.highlighted = 0;
        for hwnd in self.styled.drain() {
            if win::exists(hwnd) {
                win::set_corners(hwnd, win::CORNER_DEFAULT);
                win::set_border(hwnd, win::COLOR_DEFAULT);
            }
        }
        for (hwnd, _) in self.clipped.drain() {
            if win::exists(hwnd) && !win::is_hung(hwnd) {
                win::unclip(hwnd);
            }
        }
    }

    // -- events -------------------------------------------------------------

    fn flush(&mut self) {
        let pending = PENDING.with(|p| mem::take(&mut *p.borrow_mut()));
        PENDING_SINCE.set(None);
        // A window closed mid-drag never reports the drag ending.
        MOVING.with(|m| m.borrow_mut().retain(|&h| win::exists(h)));
        DRAGGED.with(|d| d.borrow_mut().retain(|_, at| at.elapsed() < DRAG_TO_TOP));
        for (hwnd, may_tidy) in pending {
            if !win::exists(hwnd) {
                self.forget(hwnd);
                continue;
            }
            if may_tidy {
                self.auto_tidy(hwnd);
            }
            self.style(hwnd);
        }
        // A window that was just maximised or closed gives the highlight up.
        self.sync_focus();
        if self.styled.len() > 256 {
            self.styled.retain(|&h| win::exists(h));
        }
    }

    fn forget(&mut self, hwnd: isize) {
        self.styled.remove(&hwnd);
        self.clipped.remove(&hwnd);
        if self.highlighted == hwnd {
            self.highlighted = 0;
        }
        self.placed.remove(&hwnd);
        self.live.remove(&hwnd);
    }

    /// Give a window that was just snapped or maximised its gaps.
    fn auto_tidy(&mut self, hwnd: isize) {
        let (auto, pad_max) = (self.cfg.auto_tidy, self.cfg.pad_maximized);
        // Windows following a slider drag already have somewhere to be.
        if !(auto || pad_max) || self.live.contains_key(&hwnd) || !win::is_resizable(hwnd) || !self.can_arrange(hwnd) {
            return;
        }
        let (Some(frame), Some(mon)) = (win::frame(hwnd), win::monitor(hwnd)) else { return };
        let layout = if win::is_maximized(hwnd) {
            // Dragging a window to the top of the screen is a snap like any
            // other, so tidying covers it even when maximize is left alone.
            let dragged = DRAGGED.with(|d| d.borrow_mut().remove(&hwnd)).is_some_and(|at| at.elapsed() < DRAG_TO_TOP);
            (pad_max || (auto && dragged)).then_some(Layout::Full)
        } else if auto {
            let (m, g) = spacing(&self.cfg, mon.scale);
            let tol = (SNAP_TOLERANCE * mon.scale).round() as i32;
            Layout::snapped(frame.visible, mon.work, m, g, tol)
        } else {
            None
        };
        let Some(layout) = layout else { return };
        // Some windows can't take the size we ask for (a minimum size, a
        // fixed aspect), and each attempt fires another event. Don't keep
        // asking.
        let (m, g) = spacing(&self.cfg, mon.scale);
        let target = layout.bounds(mon.work, m, g);
        if let Some((last, at)) = self.placed.get(&hwnd) {
            if *last == target && at.elapsed() < Duration::from_millis(1500) {
                return;
            }
        }
        self.put(hwnd, layout);
        self.placed.insert(hwnd, (target, Instant::now()));
        if self.placed.len() > 128 {
            self.placed.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(5));
        }
    }

    // -- arranging ----------------------------------------------------------

    fn run(&mut self, action: Action) {
        if action == Action::RoundAll {
            self.unstyle_all();
            self.style_all();
            return;
        }
        let fg = win::foreground();
        match action {
            Action::TidyWindow => self.tidy(fg),
            Action::TidyMonitor => {
                let Some(mon) = win::monitor(fg) else { return };
                for hwnd in win::top_level_windows() {
                    if win::monitor(hwnd).is_some_and(|m| m.handle == mon.handle) {
                        self.tidy(hwnd);
                    }
                }
            }
            Action::TidyAll => {
                for hwnd in win::top_level_windows() {
                    self.tidy(hwnd);
                }
            }
            Action::SnapLeft => self.snap(fg, Layout::next_left),
            Action::SnapRight => self.snap(fg, Layout::next_right),
            Action::SnapFull => self.snap(fg, |_| Layout::Full),
            Action::SnapCenter => self.snap(fg, |_| Layout::Center),
            Action::RoundAll => unreachable!(),
        }
    }

    /// Fit a window into whichever layout it's closest to.
    fn tidy(&mut self, hwnd: isize) {
        if !self.can_arrange(hwnd) {
            return;
        }
        let (Some(frame), Some(mon)) = (win::frame(hwnd), win::monitor(hwnd)) else { return };
        // Judge the layout before un-snapping, which moves the window.
        let layout = Layout::guess(frame.visible, mon.work, win::is_maximized(hwnd));
        self.put(hwnd, layout);
    }

    fn snap(&mut self, hwnd: isize, choose: fn(Option<Layout>) -> Layout) {
        if !self.can_arrange(hwnd) {
            return;
        }
        let (Some(frame), Some(mon)) = (win::frame(hwnd), win::monitor(hwnd)) else { return };
        let (m, g) = spacing(&self.cfg, mon.scale);
        let current = (!win::is_maximized(hwnd)).then(|| Layout::current(frame.visible, mon.work, m, g)).flatten();
        self.put(hwnd, choose(current));
    }

    /// Move a window into `layout` on its monitor, with gaps.
    fn put(&mut self, hwnd: isize, layout: Layout) {
        let Some(mon) = win::monitor(hwnd) else { return };
        let (m, g) = spacing(&self.cfg, mon.scale);
        let target = layout.bounds(mon.work, m, g);

        // Restoring changes the invisible border, so measure afterwards.
        win::unsnap(hwnd);
        let Some(frame) = win::frame(hwnd) else { return };
        if frame.visible.near(&target, 0) {
            return;
        }
        win::place(hwnd, target, frame, true);
        // The invisible border can change size along with the window, which
        // leaves the first move a pixel out. Measure again and correct once.
        if let Some(after) = win::frame(hwnd) {
            if !after.visible.near(&target, 0) {
                win::place(hwnd, target, after, true);
            }
        }
    }
}

/// An app with a window open, for the settings window's app picker.
#[derive(serde::Serialize)]
pub struct RunningApp {
    pub exe: String,
    pub title: String,
}

pub fn running_apps() -> Vec<RunningApp> {
    let mut seen = HashSet::new();
    let mut apps: Vec<RunningApp> = win::top_level_windows()
        .into_iter()
        .filter(|&hwnd| win::is_app_window(hwnd))
        .filter_map(|hwnd| {
            let exe = win::process_name(win::process_id(hwnd))?;
            seen.insert(exe.clone()).then(|| RunningApp { exe, title: win::title(hwnd) })
        })
        .collect();
    apps.sort_by(|a, b| a.exe.cmp(&b.exe));
    apps
}

/// The front window on each screen, by monitor.
fn front_windows(screens: usize) -> HashMap<isize, isize> {
    let mut fronts = HashMap::new();
    // Top to bottom, so the first one found on a screen is its front.
    for hwnd in win::top_level_windows() {
        if win::dim_anchor(hwnd) == Some(hwnd) {
            if let Some(monitor) = win::monitor(hwnd) {
                fronts.entry(monitor.handle).or_insert(hwnd);
                if fronts.len() == screens {
                    break;
                }
            }
        }
    }
    fronts
}

/// Margin and gap in physical pixels for a monitor's scale.
fn spacing(cfg: &Config, scale: f64) -> (i32, i32) {
    let px = |v: u32| (v as f64 * scale).round() as i32;
    (px(cfg.margin), px(cfg.gap))
}
