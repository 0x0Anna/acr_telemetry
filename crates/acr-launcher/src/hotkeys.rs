//! Hotkeys tab (section 2 of `docs/plans/acr-launcher-phase2.md`, extended
//! to cover Track Match too): binds a single "Toggle" action per target
//! — start if idle, stop if running, the typical button-box/keybind
//! convention rather than separate Start/Stop bindings — to a keyboard
//! shortcut (registered OS-wide via `global-hotkey`, so it fires even
//! while the game window has focus) and/or a controller/button-box
//! button (polled via `gilrs`). Two independent targets: Recording and
//! Track Match (live mode), each with its own keyboard + controller
//! binding.
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
//! button)…" is active for a target, the very next `ButtonPressed` event
//! is captured as that target's binding.
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
//! Firing a binding doesn't call `invoke_*_start`/`_stop` directly — it
//! checks the target's `*-running` property and picks whichever makes
//! sense, so one press/button always does the opposite of the current
//! state (the same convention a real button box's "record"/"track match"
//! button follows).

use std::cell::RefCell;
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

/// A bindable toggle action. Adding a new one means: a variant here, a
/// `label`/`config_key` string, and `is_running`/`toggle` implementations
/// — the bind/clear/listen plumbing in `init` and the background
/// listener threads are already generic over this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Target {
    Recording,
    TrackMatch,
}

impl Target {
    const ALL: [Target; 2] = [Target::Recording, Target::TrackMatch];

    fn label(self) -> &'static str {
        match self {
            Target::Recording => "Toggle Recording",
            Target::TrackMatch => "Toggle Track Match",
        }
    }

    /// TOML key prefix in `acr_launcher_hotkeys.toml` (`<prefix>_key`,
    /// `<prefix>_button`).
    fn config_key(self) -> &'static str {
        match self {
            Target::Recording => "recording",
            Target::TrackMatch => "track_match",
        }
    }

    fn is_running(self, window: &AppWindow) -> bool {
        match self {
            Target::Recording => window.get_recorder_running(),
            Target::TrackMatch => window.get_track_match_running(),
        }
    }

    /// Start if idle, stop if running — same check a real button box's
    /// single toggle button would need.
    fn toggle(self, window: &AppWindow) {
        let running = self.is_running(window);
        match (self, running) {
            (Target::Recording, false) => window.invoke_recorder_start(),
            (Target::Recording, true) => window.invoke_recorder_stop(),
            (Target::TrackMatch, false) => window.invoke_track_match_start(),
            (Target::TrackMatch, true) => window.invoke_track_match_stop(),
        }
    }
}

/// On-disk shape of `acr_launcher_hotkeys.toml`. The keyboard binding is
/// stored as `HotKey`'s own `Display`/`FromStr` string form (e.g.
/// `"control+F1"`); the controller binding stores the gamepad index
/// (gilrs's `GamepadId` only round-trips *to* `usize`, not from it, so
/// the index is tracked separately) plus the `Button` variant's `Debug`
/// name. Flat fields (not a map) so the TOML stays hand-editable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct HotkeyFileConfig {
    #[serde(default)]
    recording_key: Option<String>,
    #[serde(default)]
    recording_button: Option<ButtonBindingCfg>,
    #[serde(default)]
    track_match_key: Option<String>,
    #[serde(default)]
    track_match_button: Option<ButtonBindingCfg>,
}

impl HotkeyFileConfig {
    fn key(&self, target: Target) -> Option<&str> {
        match target {
            Target::Recording => self.recording_key.as_deref(),
            Target::TrackMatch => self.track_match_key.as_deref(),
        }
    }

    fn set_key(&mut self, target: Target, value: Option<String>) {
        match target {
            Target::Recording => self.recording_key = value,
            Target::TrackMatch => self.track_match_key = value,
        }
    }

    fn button(&self, target: Target) -> Option<&ButtonBindingCfg> {
        match target {
            Target::Recording => self.recording_button.as_ref(),
            Target::TrackMatch => self.track_match_button.as_ref(),
        }
    }

    fn set_button(&mut self, target: Target, value: Option<ButtonBindingCfg>) {
        match target {
            Target::Recording => self.recording_button = value,
            Target::TrackMatch => self.track_match_button = value,
        }
    }
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
    let file_cfg = Rc::new(RefCell::new(crate::launcher_config::load().hotkeys));

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

    // `HotKey` is `Copy` (no heap data), so the currently-registered
    // keyboard bindings live directly in an `Arc<Mutex<..>>` shared
    // between the UI-thread bind/clear callbacks and the background
    // listener thread — no separate mirror needed. Keyed by `Target`
    // rather than one field per target so the listener loop can stay
    // generic.
    let key_bindings: Arc<Mutex<std::collections::HashMap<Target, HotKey>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let button_bindings: Arc<Mutex<std::collections::HashMap<Target, (usize, Button)>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let listening: Arc<Mutex<Option<Target>>> = Arc::new(Mutex::new(None));

    // Register whatever was loaded from disk.
    {
        let cfg = file_cfg.borrow();
        for target in Target::ALL {
            if let Some(manager) = &manager {
                let mut kb = key_bindings.lock().unwrap();
                apply_saved_key_binding(manager, &mut kb, target, cfg.key(target));
            }
            if let Some(binding) = cfg.button(target).and_then(cfg_to_button) {
                button_bindings.lock().unwrap().insert(target, binding);
            }
        }
    }

    sync_ui(window, &file_cfg.borrow());

    for target in Target::ALL {
        // --- Set/Clear keyboard binding ---
        {
            let window_weak = window.as_weak();
            let file_cfg = file_cfg.clone();
            let manager = manager.clone();
            let key_bindings = key_bindings.clone();
            let set_cb = move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                set_key_binding(&window, target, &file_cfg, manager.as_deref(), &key_bindings);
            };
            match target {
                Target::Recording => window.on_hotkeys_set_recording_key(set_cb),
                Target::TrackMatch => window.on_hotkeys_set_track_match_key(set_cb),
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
                clear_key_binding(&window, target, &file_cfg, manager.as_deref(), &key_bindings);
            };
            match target {
                Target::Recording => window.on_hotkeys_clear_recording_key(clear_cb),
                Target::TrackMatch => window.on_hotkeys_clear_track_match_key(clear_cb),
            }
        }
        // --- Listen/Clear controller binding ---
        {
            let window_weak = window.as_weak();
            let listening = listening.clone();
            let listen_cb = move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                *listening.lock().unwrap() = Some(target);
                window.set_hotkeys_listening_text(
                    format!("Listening… press a controller button to bind \"{}\".", target.label())
                        .into(),
                );
            };
            match target {
                Target::Recording => window.on_hotkeys_listen_recording_button(listen_cb),
                Target::TrackMatch => window.on_hotkeys_listen_track_match_button(listen_cb),
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
                button_bindings.lock().unwrap().remove(&target);
                let mut cfg = file_cfg.borrow_mut();
                cfg.set_button(target, None);
                save_bindings(&cfg);
                sync_ui(&window, &cfg);
            };
            match target {
                Target::Recording => window.on_hotkeys_clear_recording_button(clear_button_cb),
                Target::TrackMatch => window.on_hotkeys_clear_track_match_button(clear_button_cb),
            }
        }
    }

    // Fired (via `invoke_from_event_loop`) by the `gilrs` poll thread once
    // it captures the next button press for whichever target is in
    // "listening" mode — see `spawn_gilrs_poll`. Runs on the UI thread,
    // so it's free to touch `file_cfg`'s `Rc<RefCell<...>>` directly.
    {
        let window_weak = window.as_weak();
        let file_cfg = file_cfg.clone();
        window.on_hotkeys_controller_captured(move |target_str, gamepad_index, button_name| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let target = if target_str.as_str() == Target::TrackMatch.config_key() {
                Target::TrackMatch
            } else {
                Target::Recording
            };
            let mut cfg = file_cfg.borrow_mut();
            cfg.set_button(
                target,
                Some(ButtonBindingCfg {
                    gamepad_index: gamepad_index.max(0) as usize,
                    button: button_name.to_string(),
                }),
            );
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

/// Build a `HotKey` from `target`'s modifier checkboxes + key dropdown,
/// and (re)register it: unregisters whichever `HotKey` was previously
/// bound to `target` first (a no-op if none was), then registers the new
/// one. Persists to disk and refreshes the tab's label regardless of
/// whether OS registration succeeded, so a bad binding (e.g. already
/// claimed by another app) still round-trips through the UI with a
/// status message instead of silently no-opping.
fn set_key_binding(
    window: &AppWindow,
    target: Target,
    file_cfg: &Rc<RefCell<HotkeyFileConfig>>,
    manager: Option<&GlobalHotKeyManager>,
    key_bindings: &Arc<Mutex<std::collections::HashMap<Target, HotKey>>>,
) {
    let (ctrl, alt, shift, key_choice) = match target {
        Target::Recording => (
            window.get_hotkeys_recording_mod_ctrl(),
            window.get_hotkeys_recording_mod_alt(),
            window.get_hotkeys_recording_mod_shift(),
            window.get_hotkeys_recording_key_choice(),
        ),
        Target::TrackMatch => (
            window.get_hotkeys_track_match_mod_ctrl(),
            window.get_hotkeys_track_match_mod_alt(),
            window.get_hotkeys_track_match_mod_shift(),
            window.get_hotkeys_track_match_key_choice(),
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
        let mut bindings = key_bindings.lock().unwrap();
        if let Some(old) = bindings.remove(&target) {
            let _ = manager.unregister(old);
        }
        match manager.register(hotkey) {
            Ok(()) => {
                bindings.insert(target, hotkey);
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
    cfg.set_key(target, Some(hotkey.into_string()));
    save_bindings(&cfg);
    sync_ui(window, &cfg);
}

fn clear_key_binding(
    window: &AppWindow,
    target: Target,
    file_cfg: &Rc<RefCell<HotkeyFileConfig>>,
    manager: Option<&GlobalHotKeyManager>,
    key_bindings: &Arc<Mutex<std::collections::HashMap<Target, HotKey>>>,
) {
    if let Some(manager) = manager {
        if let Some(old) = key_bindings.lock().unwrap().remove(&target) {
            let _ = manager.unregister(old);
        }
    }

    let mut cfg = file_cfg.borrow_mut();
    cfg.set_key(target, None);
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
    bindings: &mut std::collections::HashMap<Target, HotKey>,
    target: Target,
    key_str: Option<&str>,
) {
    let Some(key_str) = key_str else {
        return;
    };
    match HotKey::from_str(key_str) {
        Ok(hotkey) => match manager.register(hotkey) {
            Ok(()) => {
                bindings.insert(target, hotkey);
            }
            Err(e) => {
                eprintln!("hotkeys: failed to register saved {target:?} binding {key_str:?}: {e}");
            }
        },
        Err(e) => {
            eprintln!("hotkeys: failed to parse saved {target:?} binding {key_str:?}: {e}");
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
/// `cfg`, for both targets. Called after every successful bind/clear and
/// once at startup.
fn sync_ui(window: &AppWindow, cfg: &HotkeyFileConfig) {
    window.set_hotkeys_recording_key_label(label_or_unbound(cfg.key(Target::Recording)).into());
    window.set_hotkeys_recording_button_label(button_label(cfg.button(Target::Recording)).into());
    window.set_hotkeys_track_match_key_label(label_or_unbound(cfg.key(Target::TrackMatch)).into());
    window.set_hotkeys_track_match_button_label(button_label(cfg.button(Target::TrackMatch)).into());
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
/// `global_hotkey`'s design) and, for each `Pressed` event matching a
/// currently-bound target's `HotKey`, toggle that target.
fn spawn_hotkey_listener(
    window_weak: Weak<AppWindow>,
    key_bindings: Arc<Mutex<std::collections::HashMap<Target, HotKey>>>,
) {
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
            let matched = {
                let bindings = key_bindings.lock().unwrap();
                bindings
                    .iter()
                    .find(|(_, h)| h.id() == event.id())
                    .map(|(t, _)| *t)
            };
            let Some(target) = matched else {
                continue;
            };
            let window_weak = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    target.toggle(&window);
                }
            });
        }
    });
}

/// Background thread: poll `gilrs::Gilrs::next_event()` roughly every
/// 20ms. In "listening" mode, the next `ButtonPressed` is captured as the
/// binding for whichever target is being bound (and posted back to the
/// UI thread to persist + relabel); otherwise, a `ButtonPressed` matching
/// a currently-bound target's `(gamepad index, Button)` toggles that
/// target.
///
/// `Gilrs::new()` can fail in a headless/sandboxed environment with no
/// input devices/backends available — logged and the thread exits rather
/// than panicking, leaving controller support simply inactive.
fn spawn_gilrs_poll(
    window_weak: Weak<AppWindow>,
    listening: Arc<Mutex<Option<Target>>>,
    button_bindings: Arc<Mutex<std::collections::HashMap<Target, (usize, Button)>>>,
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

                let capture_target = listening.lock().unwrap().take();
                if let Some(target) = capture_target {
                    button_bindings
                        .lock()
                        .unwrap()
                        .insert(target, (gamepad_index, button));
                    let window_weak = window_weak.clone();
                    let button_name = format!("{button:?}");
                    let target_key = target.config_key();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = window_weak.upgrade() {
                            window.invoke_hotkeys_controller_captured(
                                target_key.into(),
                                gamepad_index as i32,
                                button_name.into(),
                            );
                        }
                    });
                    continue;
                }

                let matched = {
                    let bindings = button_bindings.lock().unwrap();
                    bindings
                        .iter()
                        .find(|(_, b)| **b == (gamepad_index, button))
                        .map(|(t, _)| *t)
                };
                if let Some(target) = matched {
                    let window_weak = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = window_weak.upgrade() {
                            target.toggle(&window);
                        }
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

/// Persist `cfg` into the shared `acr_launcher.toml`'s `[hotkeys]` table
/// (see `launcher_config.rs`). Re-loads the full launcher config first
/// so a future sibling settings section isn't clobbered by a hotkeys-only
/// write.
fn save_bindings(cfg: &HotkeyFileConfig) {
    let mut full = crate::launcher_config::load();
    full.hotkeys = cfg.clone();
    crate::launcher_config::save(&full);
}
