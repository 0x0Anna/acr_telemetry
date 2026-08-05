//! Hotkeys tab (section 2 of `docs/plans/acr-launcher-phase2.md`): binds
//! the existing Record tab's Start/Stop actions to a keyboard shortcut
//! (registered OS-wide via `global-hotkey`, so it fires even while the
//! game window has focus) and/or a controller/button-box button (polled
//! via `gilrs`).
//!
//! **Keyboard binding UX deviation from "press any key to capture":**
//! `global-hotkey` (Tauri's crate) only exposes *registering* a
//! `(Modifiers, Code)` pair for OS-level hotkey delivery — it has no API
//! for listening to arbitrary raw keypresses for capture purposes (there's
//! no lower-level "next key" channel; `GlobalHotKeyEvent::receiver()` only
//! fires for combos that are already registered). So instead of a
//! press-to-capture flow, the keyboard side uses the plan's documented
//! fallback: modifier checkboxes (Ctrl/Alt/Shift) plus a dropdown of
//! common keys, turned into a `HotKey` via `global_hotkey::hotkey::HotKey`'s
//! own `FromStr` parser (`"control+F1"` etc. — see `hotkey.rs`'s
//! `parse_hotkey`) and (re-)registered on "Set".
//!
//! **Controller binding** *is* true press-to-capture: `gilrs::Gilrs` is
//! polled on its own background thread, and while a "Bind (press
//! button)…" is active for an action, the very next `ButtonPressed` event
//! is captured as that action's binding.
//!
//! Both listeners post into the UI thread via
//! `slint::invoke_from_event_loop`, mirroring `recorder_panel.rs`'s
//! `start_recording`/`handle_child_output` pattern — Slint window handles
//! aren't `Send`, so only `Weak<AppWindow>` (which is `Send`) and plain
//! data cross the thread boundary; the actual `Rc<RefCell<...>>` bindings
//! state is only ever touched from callbacks Slint invokes on the UI
//! thread (`window.on_*`), same as `recorder_panel.rs` touches
//! `AppState`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gilrs::{Button, Event, EventType, Gilrs};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Weak};

use crate::{AppState, AppWindow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
        }
    }
}

/// On-disk shape of `acr_launcher_hotkeys.toml`. Keyboard bindings are
/// stored as `HotKey`'s own `Display`/`FromStr` string form (e.g.
/// `"control+F1"`); controller bindings store the gamepad index (gilrs's
/// `GamepadId` only round-trips *to* `usize`, not from it, so the index is
/// tracked separately) plus the `Button` variant's `Debug` name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HotkeyFileConfig {
    #[serde(default)]
    start_key: Option<String>,
    #[serde(default)]
    stop_key: Option<String>,
    #[serde(default)]
    start_button: Option<ButtonBindingCfg>,
    #[serde(default)]
    stop_button: Option<ButtonBindingCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ButtonBindingCfg {
    gamepad_index: usize,
    button: String,
}

/// Currently-registered keyboard `HotKey`s, kept around so a rebind can
/// `unregister` the previous one before registering the new one, and so
/// the event-listener thread can match an incoming `GlobalHotKeyEvent`'s
/// id back to an action. `HotKey` is `Copy` (no heap data), so this lives
/// in an `Arc<Mutex<..>>` shared directly between the UI-thread bind/clear
/// callbacks and the background listener thread — no separate mirror
/// needed.
#[derive(Default)]
struct KeyBindings {
    start: Option<HotKey>,
    stop: Option<HotKey>,
}

/// Currently-active controller bindings, read by the `gilrs` poll thread
/// on every `ButtonPressed` event to decide whether to fire the action.
#[derive(Default, Clone)]
struct ButtonBindings {
    start: Option<(usize, Button)>,
    stop: Option<(usize, Button)>,
}

pub(crate) fn init(window: &AppWindow, _state: Rc<RefCell<AppState>>) {
    // `_state` isn't touched: this panel's persisted data (bindings)
    // doesn't live in `AppState::config` (it's not part of
    // `acr_recorder.toml`), but `init` still takes the same
    // `(window, state)` signature as `export_panel`/`recorder_panel` for
    // consistency, per the plan.
    let file_cfg = Rc::new(RefCell::new(load_bindings()));

    // Per `global_hotkey`'s docs: the manager must be created on the same
    // thread that will run the (win32) event loop — that's this thread,
    // since `main()` calls `window.run()` on it after `init` returns.
    // Creation can fail in headless/sandboxed environments (no message-only
    // window support, etc.) — log and continue with keyboard hotkeys
    // simply inactive rather than panicking.
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => Some(Rc::new(m)),
        Err(e) => {
            eprintln!("hotkeys: GlobalHotKeyManager::new() failed, keyboard hotkeys disabled: {e}");
            None
        }
    };

    let key_bindings: Arc<Mutex<KeyBindings>> = Arc::new(Mutex::new(KeyBindings::default()));
    let button_bindings: Arc<Mutex<ButtonBindings>> = Arc::new(Mutex::new(ButtonBindings::default()));
    let listening: Arc<Mutex<Option<Action>>> = Arc::new(Mutex::new(None));

    // Register whatever was loaded from disk.
    {
        let cfg = file_cfg.borrow();
        if let Some(manager) = &manager {
            let mut ids = key_bindings.lock().unwrap();
            apply_saved_key_binding(manager, &mut ids, Action::Start, cfg.start_key.as_deref());
            apply_saved_key_binding(manager, &mut ids, Action::Stop, cfg.stop_key.as_deref());
        }
        let mut bb = button_bindings.lock().unwrap();
        bb.start = cfg.start_button.as_ref().and_then(cfg_to_button);
        bb.stop = cfg.stop_button.as_ref().and_then(cfg_to_button);
    }

    sync_ui(window, &file_cfg.borrow());

    // --- Set/Clear keyboard binding callbacks ---
    for action in [Action::Start, Action::Stop] {
        {
            let window_weak = window.as_weak();
            let file_cfg = file_cfg.clone();
            let manager = manager.clone();
            let key_bindings = key_bindings.clone();
            let set_cb = move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                set_key_binding(&window, action, &file_cfg, manager.as_deref(), &key_bindings);
            };
            match action {
                Action::Start => window.on_hotkeys_set_start_key(set_cb),
                Action::Stop => window.on_hotkeys_set_stop_key(set_cb),
            }
        }
        {
            let window_weak = window.as_weak();
            let file_cfg = file_cfg.clone();
            let manager = manager.clone();
            let key_bindings = key_bindings.clone();
            let clear_cb = move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                clear_key_binding(&window, action, &file_cfg, manager.as_deref(), &key_bindings);
            };
            match action {
                Action::Start => window.on_hotkeys_clear_start_key(clear_cb),
                Action::Stop => window.on_hotkeys_clear_stop_key(clear_cb),
            }
        }
        {
            let window_weak = window.as_weak();
            let listening = listening.clone();
            let listen_cb = move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                *listening.lock().unwrap() = Some(action);
                let label = match action {
                    Action::Start => "Start Recording",
                    Action::Stop => "Stop Recording",
                };
                window.set_hotkeys_listening_text(
                    format!("Listening… press a controller button to bind \"{label}\".").into(),
                );
            };
            match action {
                Action::Start => window.on_hotkeys_listen_start_button(listen_cb),
                Action::Stop => window.on_hotkeys_listen_stop_button(listen_cb),
            }
        }
        {
            let window_weak = window.as_weak();
            let file_cfg = file_cfg.clone();
            let button_bindings = button_bindings.clone();
            let clear_button_cb = move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                {
                    let mut bb = button_bindings.lock().unwrap();
                    match action {
                        Action::Start => bb.start = None,
                        Action::Stop => bb.stop = None,
                    }
                }
                let mut cfg = file_cfg.borrow_mut();
                match action {
                    Action::Start => cfg.start_button = None,
                    Action::Stop => cfg.stop_button = None,
                }
                save_bindings(&cfg);
                sync_ui(&window, &cfg);
            };
            match action {
                Action::Start => window.on_hotkeys_clear_start_button(clear_button_cb),
                Action::Stop => window.on_hotkeys_clear_stop_button(clear_button_cb),
            }
        }
    }

    // Fired (via `invoke_from_event_loop`) by the `gilrs` poll thread once
    // it captures the next button press while `listening` is set — see
    // `spawn_gilrs_poll`. Runs on the UI thread, so it's free to touch
    // `file_cfg`'s `Rc<RefCell<...>>` directly.
    {
        let window_weak = window.as_weak();
        let file_cfg = file_cfg.clone();
        window.on_hotkeys_controller_captured(move |action_str, gamepad_index, button_name| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let action = if action_str.as_str() == Action::Start.as_str() {
                Action::Start
            } else {
                Action::Stop
            };
            let mut cfg = file_cfg.borrow_mut();
            let binding = ButtonBindingCfg {
                gamepad_index: gamepad_index.max(0) as usize,
                button: button_name.to_string(),
            };
            match action {
                Action::Start => cfg.start_button = Some(binding),
                Action::Stop => cfg.stop_button = Some(binding),
            }
            save_bindings(&cfg);
            window.set_hotkeys_listening_text("".into());
            sync_ui(&window, &cfg);
        });
    }

    if manager.is_some() {
        spawn_hotkey_listener(window.as_weak(), key_bindings);
    }
    spawn_gilrs_poll(window.as_weak(), listening, button_bindings);
}

/// Build a `HotKey` from the Hotkeys tab's modifier checkboxes + key
/// dropdown for `action`, and (re)register it: unregisters whichever
/// `HotKey` was previously bound to `action` first (a no-op if none was),
/// then registers the new one. Persists to disk and refreshes the tab's
/// labels regardless of whether OS registration succeeded, so a bad
/// binding (e.g. already claimed by another app) still round-trips
/// through the UI with a status message instead of silently no-opping.
fn set_key_binding(
    window: &AppWindow,
    action: Action,
    file_cfg: &Rc<RefCell<HotkeyFileConfig>>,
    manager: Option<&GlobalHotKeyManager>,
    key_bindings: &Arc<Mutex<KeyBindings>>,
) {
    let (ctrl, alt, shift, key_choice) = match action {
        Action::Start => (
            window.get_hotkeys_start_mod_ctrl(),
            window.get_hotkeys_start_mod_alt(),
            window.get_hotkeys_start_mod_shift(),
            window.get_hotkeys_start_key_choice(),
        ),
        Action::Stop => (
            window.get_hotkeys_stop_mod_ctrl(),
            window.get_hotkeys_stop_mod_alt(),
            window.get_hotkeys_stop_mod_shift(),
            window.get_hotkeys_stop_key_choice(),
        ),
    };

    let combo = build_combo_string(ctrl, alt, shift, &key_choice);
    let hotkey = match HotKey::from_str(&combo) {
        Ok(h) => h,
        Err(e) => {
            window.set_hotkeys_status_text(format!("Invalid key combo \"{combo}\": {e}").into());
            return;
        }
    };

    if let Some(manager) = manager {
        let mut ids = key_bindings.lock().unwrap();
        let old = match action {
            Action::Start => ids.start.take(),
            Action::Stop => ids.stop.take(),
        };
        if let Some(old) = old {
            let _ = manager.unregister(old);
        }
        match manager.register(hotkey) {
            Ok(()) => {
                match action {
                    Action::Start => ids.start = Some(hotkey),
                    Action::Stop => ids.stop = Some(hotkey),
                }
                window.set_hotkeys_status_text("".into());
            }
            Err(e) => {
                window.set_hotkeys_status_text(format!("Failed to register hotkey: {e}").into());
                return;
            }
        }
    } else {
        window.set_hotkeys_status_text(
            "Global hotkey manager unavailable this session — binding saved but not active."
                .into(),
        );
    }

    let mut cfg = file_cfg.borrow_mut();
    let combo_string = hotkey.into_string();
    match action {
        Action::Start => cfg.start_key = Some(combo_string),
        Action::Stop => cfg.stop_key = Some(combo_string),
    }
    save_bindings(&cfg);
    sync_ui(window, &cfg);
}

fn clear_key_binding(
    window: &AppWindow,
    action: Action,
    file_cfg: &Rc<RefCell<HotkeyFileConfig>>,
    manager: Option<&GlobalHotKeyManager>,
    key_bindings: &Arc<Mutex<KeyBindings>>,
) {
    if let Some(manager) = manager {
        let mut ids = key_bindings.lock().unwrap();
        let old = match action {
            Action::Start => ids.start.take(),
            Action::Stop => ids.stop.take(),
        };
        if let Some(old) = old {
            let _ = manager.unregister(old);
        }
    }

    let mut cfg = file_cfg.borrow_mut();
    match action {
        Action::Start => cfg.start_key = None,
        Action::Stop => cfg.stop_key = None,
    }
    save_bindings(&cfg);
    sync_ui(window, &cfg);
}

fn build_combo_string(ctrl: bool, alt: bool, shift: bool, key: &str) -> String {
    let mut parts = Vec::new();
    if ctrl {
        parts.push("control");
    }
    if alt {
        parts.push("alt");
    }
    if shift {
        parts.push("shift");
    }
    parts.push(key);
    parts.join("+")
}

/// Register a `HotKey` loaded from disk at startup, logging (not
/// panicking) on a parse or registration failure — a hand-edited or
/// stale `acr_launcher_hotkeys.toml` shouldn't stop the launcher from
/// starting.
fn apply_saved_key_binding(
    manager: &GlobalHotKeyManager,
    ids: &mut KeyBindings,
    action: Action,
    key_str: Option<&str>,
) {
    let Some(key_str) = key_str else {
        return;
    };
    match HotKey::from_str(key_str) {
        Ok(hotkey) => match manager.register(hotkey) {
            Ok(()) => match action {
                Action::Start => ids.start = Some(hotkey),
                Action::Stop => ids.stop = Some(hotkey),
            },
            Err(e) => {
                eprintln!(
                    "hotkeys: failed to register saved {action:?} binding {key_str:?}: {e}"
                );
            }
        },
        Err(e) => {
            eprintln!("hotkeys: failed to parse saved {action:?} binding {key_str:?}: {e}");
        }
    }
}

fn cfg_to_button(cfg: &ButtonBindingCfg) -> Option<(usize, Button)> {
    parse_button_name(&cfg.button).map(|b| (cfg.gamepad_index, b))
}

fn parse_button_name(name: &str) -> Option<Button> {
    use Button::*;
    Some(match name {
        "South" => South,
        "East" => East,
        "North" => North,
        "West" => West,
        "C" => C,
        "Z" => Z,
        "LeftTrigger" => LeftTrigger,
        "LeftTrigger2" => LeftTrigger2,
        "RightTrigger" => RightTrigger,
        "RightTrigger2" => RightTrigger2,
        "Select" => Select,
        "Start" => Start,
        "Mode" => Mode,
        "LeftThumb" => LeftThumb,
        "RightThumb" => RightThumb,
        "DPadUp" => DPadUp,
        "DPadDown" => DPadDown,
        "DPadLeft" => DPadLeft,
        "DPadRight" => DPadRight,
        "Unknown" => Unknown,
        _ => return None,
    })
}

/// Push the Hotkeys tab's read-only labels (current bindings) from
/// `cfg`. Called after every successful bind/clear and once at startup.
fn sync_ui(window: &AppWindow, cfg: &HotkeyFileConfig) {
    window.set_hotkeys_start_key_label(label_or_unbound(cfg.start_key.as_deref()).into());
    window.set_hotkeys_stop_key_label(label_or_unbound(cfg.stop_key.as_deref()).into());
    window.set_hotkeys_start_button_label(button_label(cfg.start_button.as_ref()).into());
    window.set_hotkeys_stop_button_label(button_label(cfg.stop_button.as_ref()).into());
}

fn label_or_unbound(s: Option<&str>) -> String {
    s.filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "unbound".to_string())
}

fn button_label(binding: Option<&ButtonBindingCfg>) -> String {
    match binding {
        Some(b) => format!("{} (Controller {})", b.button, b.gamepad_index),
        None => "unbound".to_string(),
    }
}

/// Background thread: block on `GlobalHotKeyEvent::receiver()` (a
/// `crossbeam_channel` shared across the whole process, per
/// `global_hotkey`'s design) and, for each `Pressed` event whose id
/// matches the currently-bound Start/Stop `HotKey`, invoke the same
/// Slint callback the Record tab's own buttons trigger.
fn spawn_hotkey_listener(window_weak: Weak<AppWindow>, key_bindings: Arc<Mutex<KeyBindings>>) {
    std::thread::spawn(move || {
        let receiver = GlobalHotKeyEvent::receiver();
        loop {
            let event = match receiver.recv() {
                Ok(event) => event,
                Err(_) => break, // channel gone (process exiting)
            };
            if event.state() != HotKeyState::Pressed {
                continue;
            }
            let action = {
                let ids = key_bindings.lock().unwrap();
                if ids.start.map(|h| h.id()) == Some(event.id()) {
                    Some(Action::Start)
                } else if ids.stop.map(|h| h.id()) == Some(event.id()) {
                    Some(Action::Stop)
                } else {
                    None
                }
            };
            let Some(action) = action else {
                continue;
            };
            let window_weak = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    match action {
                        Action::Start => window.invoke_recorder_start(),
                        Action::Stop => window.invoke_recorder_stop(),
                    }
                }
            });
        }
    });
}

/// Background thread: poll `gilrs::Gilrs::next_event()` roughly every
/// 20ms. In "listening" mode, the next `ButtonPressed` is captured as the
/// binding for whichever action is being bound (and posted back to the UI
/// thread to persist + relabel); otherwise, a `ButtonPressed` matching a
/// currently-bound `(gamepad index, Button)` fires the same Slint
/// start/stop callback the keyboard listener does.
///
/// `Gilrs::new()` can fail in a headless/sandboxed environment with no
/// input devices/backends available — logged and the thread exits rather
/// than panicking, leaving controller support simply inactive.
fn spawn_gilrs_poll(
    window_weak: Weak<AppWindow>,
    listening: Arc<Mutex<Option<Action>>>,
    button_bindings: Arc<Mutex<ButtonBindings>>,
) {
    std::thread::spawn(move || {
        let mut gilrs = match Gilrs::new() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("hotkeys: Gilrs::new() failed, controller support disabled: {e}");
                return;
            }
        };

        loop {
            while let Some(Event { id, event, .. }) = gilrs.next_event() {
                let EventType::ButtonPressed(button, _code) = event else {
                    continue;
                };
                let gamepad_index: usize = id.into();

                let capture_action = listening.lock().unwrap().take();
                if let Some(action) = capture_action {
                    {
                        let mut bb = button_bindings.lock().unwrap();
                        match action {
                            Action::Start => bb.start = Some((gamepad_index, button)),
                            Action::Stop => bb.stop = Some((gamepad_index, button)),
                        }
                    }
                    let window_weak = window_weak.clone();
                    let button_name = format!("{button:?}");
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = window_weak.upgrade() {
                            window.invoke_hotkeys_controller_captured(
                                action.as_str().into(),
                                gamepad_index as i32,
                                button_name.into(),
                            );
                        }
                    });
                    continue;
                }

                let fire_action = {
                    let bb = button_bindings.lock().unwrap();
                    if bb.start == Some((gamepad_index, button)) {
                        Some(Action::Start)
                    } else if bb.stop == Some((gamepad_index, button)) {
                        Some(Action::Stop)
                    } else {
                        None
                    }
                };
                if let Some(action) = fire_action {
                    let window_weak = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = window_weak.upgrade() {
                            match action {
                                Action::Start => window.invoke_recorder_start(),
                                Action::Stop => window.invoke_recorder_stop(),
                            }
                        }
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

/// Where `acr_launcher_hotkeys.toml` is written/read: next to the
/// launcher's own executable, same convention as
/// `recorder_panel.rs::config_file_path` for `acr_recorder.toml`.
fn config_file_path() -> PathBuf {
    acr_recorder::config::base_dir()
        .map(|dir| dir.join("acr_launcher_hotkeys.toml"))
        .unwrap_or_else(|| PathBuf::from("acr_launcher_hotkeys.toml"))
}

fn load_bindings() -> HotkeyFileConfig {
    let path = config_file_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        match toml::from_str(&text) {
            Ok(cfg) => return cfg,
            Err(e) => eprintln!("hotkeys: failed to parse {}: {e}", path.display()),
        }
    }
    HotkeyFileConfig::default()
}

fn save_bindings(cfg: &HotkeyFileConfig) {
    let path = config_file_path();
    match toml::to_string_pretty(cfg) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                eprintln!("hotkeys: failed to write {}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("hotkeys: failed to serialize hotkey bindings: {e}"),
    }
}
