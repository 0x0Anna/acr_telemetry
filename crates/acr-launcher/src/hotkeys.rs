//! Hotkeys tab (section 2 of `docs/plans/acr-launcher-phase2.md`): binds
//! a single "Toggle Recording" action — start if idle, stop if
//! running, the typical button-box/keybind convention rather than
//! separate Start/Stop bindings — to a keyboard shortcut (registered
//! OS-wide via `global-hotkey`, so it fires even while the game window
//! has focus) and/or a controller/button-box button (polled via
//! `gilrs`).
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
//! polled on its own background thread, and while "Bind (press
//! button)…" is active, the very next `ButtonPressed` event is captured
//! as the binding.
//!
//! Both listeners post into the UI thread via
//! `slint::invoke_from_event_loop`, mirroring `recorder_panel.rs`'s
//! `start_recording`/`handle_child_output` pattern — Slint window handles
//! aren't `Send`, so only `Weak<AppWindow>` (which is `Send`) and plain
//! data cross the thread boundary; the actual `Rc<RefCell<...>>` bindings
//! state is only ever touched from callbacks Slint invokes on the UI
//! thread (`window.on_*`), same as `recorder_panel.rs` touches
//! `AppState`.
//!
//! Firing the binding doesn't call `invoke_recorder_start`/`_stop`
//! directly — it checks `recorder-running` and picks whichever makes
//! sense, so one press/button always does the opposite of the current
//! state (the same convention a real button box's "record" button
//! follows).

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

/// On-disk shape of `acr_launcher_hotkeys.toml`. The keyboard binding is
/// stored as `HotKey`'s own `Display`/`FromStr` string form (e.g.
/// `"control+F1"`); the controller binding stores the gamepad index
/// (gilrs's `GamepadId` only round-trips *to* `usize`, not from it, so
/// the index is tracked separately) plus the `Button` variant's `Debug`
/// name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HotkeyFileConfig {
    #[serde(default)]
    toggle_key: Option<String>,
    #[serde(default)]
    toggle_button: Option<ButtonBindingCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ButtonBindingCfg {
    gamepad_index: usize,
    button: String,
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
    // window support, etc.) — log and continue with the keyboard hotkey
    // simply inactive rather than panicking.
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => Some(Rc::new(m)),
        Err(e) => {
            eprintln!("hotkeys: GlobalHotKeyManager::new() failed, keyboard hotkey disabled: {e}");
            None
        }
    };

    // `HotKey` is `Copy` (no heap data), so the currently-registered
    // keyboard binding lives directly in an `Arc<Mutex<..>>` shared
    // between the UI-thread bind/clear callbacks and the background
    // listener thread — no separate mirror needed.
    let key_binding: Arc<Mutex<Option<HotKey>>> = Arc::new(Mutex::new(None));
    let button_binding: Arc<Mutex<Option<(usize, Button)>>> = Arc::new(Mutex::new(None));
    let listening: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    // Register whatever was loaded from disk.
    {
        let cfg = file_cfg.borrow();
        if let Some(manager) = &manager {
            let mut kb = key_binding.lock().unwrap();
            apply_saved_key_binding(manager, &mut kb, cfg.toggle_key.as_deref());
        }
        *button_binding.lock().unwrap() = cfg.toggle_button.as_ref().and_then(cfg_to_button);
    }

    sync_ui(window, &file_cfg.borrow());

    // --- Set/Clear keyboard binding callbacks ---
    {
        let window_weak = window.as_weak();
        let file_cfg = file_cfg.clone();
        let manager = manager.clone();
        let key_binding = key_binding.clone();
        window.on_hotkeys_set_toggle_key(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            set_key_binding(&window, &file_cfg, manager.as_deref(), &key_binding);
        });
    }
    {
        let window_weak = window.as_weak();
        let file_cfg = file_cfg.clone();
        let manager = manager.clone();
        let key_binding = key_binding.clone();
        window.on_hotkeys_clear_toggle_key(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            clear_key_binding(&window, &file_cfg, manager.as_deref(), &key_binding);
        });
    }
    {
        let window_weak = window.as_weak();
        let listening = listening.clone();
        window.on_hotkeys_listen_toggle_button(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            *listening.lock().unwrap() = true;
            window.set_hotkeys_listening_text(
                "Listening… press a controller button to bind \"Toggle Recording\".".into(),
            );
        });
    }
    {
        let window_weak = window.as_weak();
        let file_cfg = file_cfg.clone();
        let button_binding = button_binding.clone();
        window.on_hotkeys_clear_toggle_button(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            *button_binding.lock().unwrap() = None;
            let mut cfg = file_cfg.borrow_mut();
            cfg.toggle_button = None;
            save_bindings(&cfg);
            sync_ui(&window, &cfg);
        });
    }

    // Fired (via `invoke_from_event_loop`) by the `gilrs` poll thread once
    // it captures the next button press while `listening` is set — see
    // `spawn_gilrs_poll`. Runs on the UI thread, so it's free to touch
    // `file_cfg`'s `Rc<RefCell<...>>` directly.
    {
        let window_weak = window.as_weak();
        let file_cfg = file_cfg.clone();
        window.on_hotkeys_controller_captured(move |gamepad_index, button_name| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let mut cfg = file_cfg.borrow_mut();
            cfg.toggle_button = Some(ButtonBindingCfg {
                gamepad_index: gamepad_index.max(0) as usize,
                button: button_name.to_string(),
            });
            save_bindings(&cfg);
            window.set_hotkeys_listening_text("".into());
            sync_ui(&window, &cfg);
        });
    }

    if manager.is_some() {
        spawn_hotkey_listener(window.as_weak(), key_binding);
    }
    spawn_gilrs_poll(window.as_weak(), listening, button_binding);
}

/// Build a `HotKey` from the Hotkeys tab's modifier checkboxes + key
/// dropdown, and (re)register it: unregisters whichever `HotKey` was
/// previously bound first (a no-op if none was), then registers the new
/// one. Persists to disk and refreshes the tab's label regardless of
/// whether OS registration succeeded, so a bad binding (e.g. already
/// claimed by another app) still round-trips through the UI with a
/// status message instead of silently no-opping.
fn set_key_binding(
    window: &AppWindow,
    file_cfg: &Rc<RefCell<HotkeyFileConfig>>,
    manager: Option<&GlobalHotKeyManager>,
    key_binding: &Arc<Mutex<Option<HotKey>>>,
) {
    let ctrl = window.get_hotkeys_toggle_mod_ctrl();
    let alt = window.get_hotkeys_toggle_mod_alt();
    let shift = window.get_hotkeys_toggle_mod_shift();
    let key_choice = window.get_hotkeys_toggle_key_choice();

    let combo = build_combo_string(ctrl, alt, shift, &key_choice);
    let hotkey = match HotKey::from_str(&combo) {
        Ok(h) => h,
        Err(e) => {
            window.set_hotkeys_status_text(format!("Invalid key combo \"{combo}\": {e}").into());
            return;
        }
    };

    if let Some(manager) = manager {
        let mut binding = key_binding.lock().unwrap();
        if let Some(old) = binding.take() {
            let _ = manager.unregister(old);
        }
        match manager.register(hotkey) {
            Ok(()) => {
                *binding = Some(hotkey);
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
    cfg.toggle_key = Some(hotkey.into_string());
    save_bindings(&cfg);
    sync_ui(window, &cfg);
}

fn clear_key_binding(
    window: &AppWindow,
    file_cfg: &Rc<RefCell<HotkeyFileConfig>>,
    manager: Option<&GlobalHotKeyManager>,
    key_binding: &Arc<Mutex<Option<HotKey>>>,
) {
    if let Some(manager) = manager {
        let mut binding = key_binding.lock().unwrap();
        if let Some(old) = binding.take() {
            let _ = manager.unregister(old);
        }
    }

    let mut cfg = file_cfg.borrow_mut();
    cfg.toggle_key = None;
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
    binding: &mut Option<HotKey>,
    key_str: Option<&str>,
) {
    let Some(key_str) = key_str else {
        return;
    };
    match HotKey::from_str(key_str) {
        Ok(hotkey) => match manager.register(hotkey) {
            Ok(()) => *binding = Some(hotkey),
            Err(e) => {
                eprintln!("hotkeys: failed to register saved toggle binding {key_str:?}: {e}");
            }
        },
        Err(e) => {
            eprintln!("hotkeys: failed to parse saved toggle binding {key_str:?}: {e}");
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
    window.set_hotkeys_toggle_key_label(label_or_unbound(cfg.toggle_key.as_deref()).into());
    window.set_hotkeys_toggle_button_label(button_label(cfg.toggle_button.as_ref()).into());
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

/// Toggle recording on the UI thread: start if idle, stop if running —
/// the same check a real button box's single "record" button would need,
/// so one keyboard/controller trigger always does the opposite of the
/// current state instead of requiring two separate bindings.
fn toggle_recording(window: &AppWindow) {
    if window.get_recorder_running() {
        window.invoke_recorder_stop();
    } else {
        window.invoke_recorder_start();
    }
}

/// Background thread: block on `GlobalHotKeyEvent::receiver()` (a
/// `crossbeam_channel` shared across the whole process, per
/// `global_hotkey`'s design) and, for each `Pressed` event matching the
/// currently-bound toggle `HotKey`, toggle recording.
fn spawn_hotkey_listener(window_weak: Weak<AppWindow>, key_binding: Arc<Mutex<Option<HotKey>>>) {
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
            let matched = key_binding
                .lock()
                .unwrap()
                .map(|h| h.id() == event.id())
                .unwrap_or(false);
            if !matched {
                continue;
            }
            let window_weak = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    toggle_recording(&window);
                }
            });
        }
    });
}

/// Background thread: poll `gilrs::Gilrs::next_event()` roughly every
/// 20ms. In "listening" mode, the next `ButtonPressed` is captured as the
/// toggle binding (and posted back to the UI thread to persist +
/// relabel); otherwise, a `ButtonPressed` matching the currently-bound
/// `(gamepad index, Button)` toggles recording.
///
/// `Gilrs::new()` can fail in a headless/sandboxed environment with no
/// input devices/backends available — logged and the thread exits rather
/// than panicking, leaving controller support simply inactive.
fn spawn_gilrs_poll(
    window_weak: Weak<AppWindow>,
    listening: Arc<Mutex<bool>>,
    button_binding: Arc<Mutex<Option<(usize, Button)>>>,
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

                let capturing = {
                    let mut listening = listening.lock().unwrap();
                    std::mem::take(&mut *listening)
                };
                if capturing {
                    *button_binding.lock().unwrap() = Some((gamepad_index, button));
                    let window_weak = window_weak.clone();
                    let button_name = format!("{button:?}");
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = window_weak.upgrade() {
                            window.invoke_hotkeys_controller_captured(
                                gamepad_index as i32,
                                button_name.into(),
                            );
                        }
                    });
                    continue;
                }

                let matched = *button_binding.lock().unwrap() == Some((gamepad_index, button));
                if matched {
                    let window_weak = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = window_weak.upgrade() {
                            toggle_recording(&window);
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
