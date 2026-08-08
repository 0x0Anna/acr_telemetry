//! Telemetry Bridge tab (phase 3): wraps `acr_telemetry_bridge.exe`, a
//! long-running server that reads ACC/AC Rally shared memory and serves
//! it over UDP and/or HTTP for a phone/second-screen dashboard (see
//! `docs/BRIDGE.md`, `src/bin/acr_telemetry_bridge.rs`). Needs the sim
//! already running and connectable when started — it does not retry, per
//! `ACCSharedMemory::new()` being called once at startup and returning
//! `Err` (process exit) if the shared memory isn't up yet.
//!
//! Start/Stop lifecycle mirrors `recorder_panel.rs`: write the UI's
//! settings into `acr_telemetry_bridge.toml` (so the spawned process picks
//! them up with no CLI flags, avoiding flag/TOML duplication), spawn
//! hidden, stream output into a log + status pill. Stop writes a stop
//! file (backend fix in `src/bin/acr_telemetry_bridge.rs` mirroring the
//! phase-2 `acr_track_match` fix) rather than killing the process — the
//! bridge is spawned with no shared console, so its own Ctrl+C handler
//! can't be reached from here.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::process;
use crate::{AppState, AppWindow};

pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    let cfg = crate::launcher_config::load().telemetry_bridge;
    window.set_telemetry_bridge_rate_hz(cfg.rate_hz.to_string().into());
    window.set_telemetry_bridge_udp_enabled(cfg.udp_enabled);
    window.set_telemetry_bridge_udp_target(cfg.udp_target.into());
    window.set_telemetry_bridge_http_enabled(cfg.http_enabled);
    window.set_telemetry_bridge_http_addr(cfg.http_addr.into());
    window.set_telemetry_bridge_temp_unit(cfg.temperature_unit.into());

    {
        let window_weak = window.as_weak();
        window.on_telemetry_bridge_start(move || {
            let Some(window) = window_weak.upgrade() else { return };

            if window.get_telemetry_bridge_running() {
                // Already have a child streaming; ignore a double-click.
                return;
            }

            start(&window);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_telemetry_bridge_stop(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let stop_path = stop_file_path();
            match std::fs::write(&stop_path, b"") {
                Ok(()) => {
                    window.set_telemetry_bridge_status_pill("Stopping…".into());
                    append_log(&window, &format!("Wrote stop file: {}", stop_path.display()));
                }
                Err(e) => {
                    window.set_telemetry_bridge_status_text(
                        format!("Failed to write stop file: {e}").into(),
                    );
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_telemetry_bridge_open_dashboard(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let addr = window.get_telemetry_bridge_http_addr().to_string();
            let url = dashboard_url(&addr);
            let _ = std::process::Command::new("explorer").arg(url).spawn();
        });
    }

    let _ = state; // no AppState fields needed by this panel today
}

/// Turn the configured `http_addr` (e.g. `"0.0.0.0:8080"`, matching
/// `docs/BRIDGE.md`'s dashboard address, or a bare `":8080"`) into a
/// browser-openable URL. `0.0.0.0` is a bind address, not something a
/// browser can navigate to reliably across platforms — swap it (or a
/// missing host) for `localhost`, since the dashboard is opened from the
/// same machine the bridge runs on.
fn dashboard_url(addr: &str) -> String {
    let addr = addr.trim();
    let host_port = if let Some(port) = addr.strip_prefix("0.0.0.0:") {
        format!("localhost:{port}")
    } else if let Some(port) = addr.strip_prefix(':') {
        format!("localhost:{port}")
    } else if addr.is_empty() {
        "localhost:8080".to_string()
    } else {
        addr.to_string()
    };
    format!("http://{host_port}")
}

/// Same path `src/bin/acr_telemetry_bridge.rs`'s own `stop_file_path()`
/// resolves to — duplicated here (rather than imported) because the
/// bridge is a standalone `src/bin/` binary, not a `lib.rs` module like
/// `acr_recorder::track_match_app`, so there's nothing for the launcher to
/// `use`. Keep both copies in sync if the convention changes.
fn stop_file_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("acr_telemetry").join("acr_telemetry_bridge_stop"))
        .unwrap_or_else(|| PathBuf::from(".acr_telemetry_bridge_stop"))
}

/// Where `acr_telemetry_bridge.toml` is written back to: next to the
/// launcher's own executable, same convention `recorder_panel.rs` uses
/// for `acr_recorder.toml`, and the first path
/// `config::load_bridge_config()` searches.
fn config_file_path() -> PathBuf {
    acr_recorder::config::base_dir()
        .map(|dir| dir.join("acr_telemetry_bridge.toml"))
        .unwrap_or_else(|| PathBuf::from("acr_telemetry_bridge.toml"))
}

/// Patches `rate_hz`/`udp_target`/`http_addr`/`temperature_unit` into the
/// on-disk `acr_telemetry_bridge.toml` via `toml_edit` on every Start,
/// rather than overwriting the whole file with a freshly serialized
/// struct. `dashboard_slots`/`telemetry_colors` (advanced, TOML-only,
/// out of scope for the v1 tab UI per the phase-3 plan — same
/// "config-first, don't expose everything" precedent as Track Match's v1
/// scope) are left completely untouched instead of being silently erased:
/// a full-struct round-trip previously had no way to preserve keys it
/// didn't know about, so any hand-added `dashboard_slots`/
/// `telemetry_colors` table was lost the next time the panel started the
/// bridge.
fn write_bridge_config(window: &AppWindow, rate_hz: u64) -> std::io::Result<PathBuf> {
    let udp_target = if window.get_telemetry_bridge_udp_enabled() {
        let t = window.get_telemetry_bridge_udp_target().to_string();
        if t.trim().is_empty() { None } else { Some(t) }
    } else {
        None
    };
    let http_addr = if window.get_telemetry_bridge_http_enabled() {
        let a = window.get_telemetry_bridge_http_addr().to_string();
        if a.trim().is_empty() { "0.0.0.0:8080".to_string() } else { a }
    } else {
        String::new()
    };
    let temperature_unit = window.get_telemetry_bridge_temp_unit().to_string();

    let path = config_file_path();
    let mut doc = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .unwrap_or_default();

    doc["rate_hz"] = toml_edit::value(rate_hz as i64);
    match udp_target {
        Some(t) => doc["udp_target"] = toml_edit::value(t),
        None => {
            doc.remove("udp_target");
        }
    }
    doc["http_addr"] = toml_edit::value(http_addr);
    doc["temperature_unit"] = toml_edit::value(temperature_unit);

    std::fs::write(&path, doc.to_string())?;
    Ok(path)
}

/// Persist the UI's settings into `acr_launcher.toml` so they pre-fill
/// next launch (independent of `acr_telemetry_bridge.toml`, which gets
/// overwritten fresh from these same values on every Start).
fn save_ui_settings(window: &AppWindow, rate_hz: u64) {
    let mut cfg = crate::launcher_config::load();
    let b = &mut cfg.telemetry_bridge;
    b.rate_hz = rate_hz;
    b.udp_enabled = window.get_telemetry_bridge_udp_enabled();
    b.udp_target = window.get_telemetry_bridge_udp_target().to_string();
    b.http_enabled = window.get_telemetry_bridge_http_enabled();
    b.http_addr = window.get_telemetry_bridge_http_addr().to_string();
    b.temperature_unit = window.get_telemetry_bridge_temp_unit().to_string();
    crate::launcher_config::save(&cfg);
}

fn start(window: &AppWindow) {
    let rate_hz_text = window.get_telemetry_bridge_rate_hz().to_string();
    let rate_hz: u64 = match rate_hz_text.trim().parse() {
        Ok(hz) if hz >= 1 => hz,
        _ => {
            window.set_telemetry_bridge_status_text(
                format!("Error: rate (Hz) \"{rate_hz_text}\" must be a whole number of 1 or more.").into(),
            );
            return;
        }
    };

    save_ui_settings(window, rate_hz);

    if let Err(e) = write_bridge_config(window, rate_hz) {
        window.set_telemetry_bridge_status_text(
            format!("Failed to write acr_telemetry_bridge.toml: {e}").into(),
        );
        return;
    }

    let binary_path = process::resolve_binary("acr_telemetry_bridge.exe");

    let child = match process::spawn_hidden(&binary_path, &[]) {
        Ok(child) => child,
        Err(e) => {
            window.set_telemetry_bridge_status_text(
                format!("Failed to start acr_telemetry_bridge.exe: {e}").into(),
            );
            return;
        }
    };

    window.set_telemetry_bridge_log("".into());
    window.set_telemetry_bridge_status_text("".into());
    window.set_telemetry_bridge_status_pill("Starting…".into());
    window.set_telemetry_bridge_running(true);
    append_log(window, "Started acr_telemetry_bridge.exe");

    let window_weak = window.as_weak();
    std::thread::spawn(move || {
        // Substring-match the exact `eprintln!` lines
        // `src/bin/acr_telemetry_bridge.rs` prints ("Bridge running at"
        // once the read loop starts, "failed to bind" on a non-fatal HTTP
        // bind failure — the UDP/state loop keeps going even then, so this
        // doesn't flip the pill to stopped, only surfaces the error in the
        // log — and this panel's own "Stop file detected" from the backend
        // fix) for the status pill, and always append the line to the raw log.
        let result = process::wait_for_output(child, |_is_stderr, line| {
            let window_weak = window_weak.clone();
            let line = line.to_string();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    update_status_pill(&window, &line);
                    append_log(&window, &line);
                }
            });
        });

        let window_weak = window_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window_weak.upgrade() {
                window.set_telemetry_bridge_running(false);
                let line = match result.exit_code {
                    Some(code) => format!("Process exited (code {code})"),
                    None => "Process exited".to_string(),
                };
                append_log(&window, &line);
                window.set_telemetry_bridge_status_pill("Stopped".into());
            }
        });
    });
}

fn update_status_pill(window: &AppWindow, line: &str) {
    if line.contains("Bridge running at") {
        window.set_telemetry_bridge_status_pill("Running".into());
    } else if line.contains("failed to bind") {
        window.set_telemetry_bridge_status_text(line.to_string().into());
    } else if line.contains("Stop file detected") {
        window.set_telemetry_bridge_status_pill("Stopping…".into());
    }
}

fn append_log(window: &AppWindow, line: &str) {
    crate::append_line!(window, get_telemetry_bridge_log, set_telemetry_bridge_log, line);
}
