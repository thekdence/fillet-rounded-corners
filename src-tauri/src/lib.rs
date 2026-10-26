mod autostart;
mod config;
mod engine;
mod layout;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use config::{Action, Config};
use engine::{Engine, RunningApp};
use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, Wry,
};

const SETTINGS: &str = "settings";

struct App {
    config: Arc<Mutex<Config>>,
    config_path: PathBuf,
    engine: Engine,
    conflicts: Arc<Mutex<Vec<Action>>>,
    pause_item: MenuItem<Wry>,
}

impl App {
    fn config(&self) -> Config {
        self.config.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn update(&self, app: &AppHandle, next: Config) -> Result<Config, String> {
        let next = next.normalized();
        next.save(&self.config_path).map_err(|e| format!("Couldn't save settings: {e}"))?;
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = next.clone();
        self.engine.reload();
        let _ = self.pause_item.set_text(pause_label(next.enabled));
        let _ = app.emit_to(SETTINGS, "config", &next);
        Ok(next)
    }
}

fn pause_label(enabled: bool) -> &'static str {
    if enabled {
        "Pause Fillet"
    } else {
        "Resume Fillet"
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    config: Config,
    conflicts: Vec<Action>,
    start_with_windows: bool,
    version: String,
}

#[tauri::command]
fn get_state(app: AppHandle, state: tauri::State<'_, App>) -> Snapshot {
    Snapshot {
        config: state.config(),
        conflicts: state.conflicts.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        start_with_windows: autostart::is_enabled(),
        version: app.package_info().version.to_string(),
    }
}

#[tauri::command]
fn set_config(app: AppHandle, state: tauri::State<'_, App>, config: Config) -> Result<Config, String> {
    state.update(&app, config)
}

#[tauri::command]
fn set_start_with_windows(enabled: bool) -> Result<bool, String> {
    autostart::set(enabled)?;
    Ok(autostart::is_enabled())
}

#[tauri::command]
fn running_apps() -> Vec<RunningApp> {
    engine::running_apps()
}

#[tauri::command]
fn run_action(state: tauri::State<'_, App>, action: Action) {
    state.engine.run(action);
}

#[tauri::command]
fn suspend_hotkeys(state: tauri::State<'_, App>, suspend: bool) {
    state.engine.suspend_hotkeys(suspend);
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// The settings window is created on demand and destroyed on close, so the
/// web view only takes memory while you're actually using it.
fn open_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::default())
        .title("Fillet")
        .inner_size(560.0, 770.0)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .shadow(true)
        .center()
        // The page shows the window once it has painted, so it never
        // flashes white. This is the fallback if it somehow doesn't.
        .visible(false)
        .build();
    if let Ok(window) = built {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            let _ = window.show();
        });
    }
}

fn toggle_pause(app: &AppHandle) {
    let state = app.state::<App>();
    let mut next = state.config();
    next.enabled = !next.enabled;
    let _ = state.update(app, next);
}

fn build_tray(app: &AppHandle, pause_item: &MenuItem<Wry>) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "tidy", "Tidy all windows", true, None::<&str>)?,
            pause_item,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit Fillet", true, None::<&str>)?,
        ],
    )?;
    TrayIconBuilder::with_id("fillet")
        .icon(app.default_window_icon().cloned().expect("app icon missing"))
        .tooltip("Fillet")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                open_settings(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "settings" => open_settings(app),
            "tidy" => app.state::<App>().engine.run(Action::TidyAll),
            "pause" => toggle_pause(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // Launching Fillet again just opens the settings of the running one.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| open_settings(app)))
        .setup(|app| {
            let handle = app.handle().clone();
            let config_path = app.path().app_config_dir()?.join("config.json");
            // "Start with Windows" is only ever turned on from the settings.
            // An unsigned app adding itself to the Run key on first launch,
            // alongside a keyboard hook, gets flagged by Microsoft Defender.
            let config = Config::load(&config_path);

            let enabled = config.enabled;
            let config = Arc::new(Mutex::new(config));
            let conflicts = Arc::new(Mutex::new(Vec::new()));
            let engine = {
                let (handle, conflicts) = (handle.clone(), conflicts.clone());
                Engine::start(
                    config.clone(),
                    Box::new(move |taken| {
                        let _ = handle.emit_to(SETTINGS, "conflicts", &taken);
                        *conflicts.lock().unwrap_or_else(|e| e.into_inner()) = taken;
                    }),
                )
            };

            let pause_item = MenuItem::with_id(app, "pause", pause_label(enabled), true, None::<&str>)?;
            build_tray(&handle, &pause_item)?;
            app.manage(App { config, config_path, engine, conflicts, pause_item });

            if !std::env::args().any(|a| a == autostart::FLAG) {
                open_settings(&handle);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_config,
            set_start_with_windows,
            running_apps,
            run_action,
            suspend_hotkeys,
            quit
        ])
        .build(tauri::generate_context!())
        .expect("error while building Fillet");

    app.run(|app, event| match event {
        // Closing the settings window must not quit; only "Quit" does.
        RunEvent::ExitRequested { api, code: None, .. } => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<App>() {
                state.engine.shutdown();
            }
        }
        _ => {}
    });
}
