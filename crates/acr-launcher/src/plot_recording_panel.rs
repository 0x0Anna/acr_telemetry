//! Plot Recording tab (phase 3): wraps `acr_plot_recording.exe`, a
//! one-shot tool that reads a physics `.rkyv` file (deriving the sibling
//! `{stem}.graphics.rkyv` itself — no separate graphics arg needed) and
//! writes a self-contained Plotly HTML plot. See
//! `src/bin/acr_plot_recording.rs`: positional args only
//! (`<physics.rkyv> [output.html]`), defaulting the output to
//! `{stem}_plot.html` next to the input if left blank. No config file —
//! purely positional-arg driven — so this panel persists only the
//! last-used input directory, in `acr_launcher.toml`'s `[plot_recording]`.
//!
//! Mirrors `export_panel.rs`'s fire-and-forget shape: Run spawns the
//! child, streams output into a log, and the process exits on its own —
//! no Start/Stop lifecycle needed.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::{ComponentHandle, Weak};

use crate::process;
use crate::{AppState, AppWindow};

pub(crate) fn init(window: &AppWindow, state: Rc<RefCell<AppState>>) {
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_plot_recording_pick_input(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let last_dir = crate::launcher_config::load().plot_recording.last_dir;
            let mut dialog = rfd::FileDialog::new().add_filter("rkyv recording", &["rkyv"]);
            if let Some(dir) = last_dir.as_ref().map(PathBuf::from).filter(|p| p.exists()) {
                dialog = dialog.set_directory(&dir);
            }
            if let Some(path) = dialog.pick_file() {
                window.set_plot_recording_input_path(path.to_string_lossy().into_owned().into());
                state.borrow_mut().plot_recording_input = Some(path.clone());

                if let Some(dir) = path.parent() {
                    let mut cfg = crate::launcher_config::load();
                    cfg.plot_recording.last_dir = Some(dir.to_string_lossy().into_owned());
                    crate::launcher_config::save(&cfg);
                }
            }
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_plot_recording_pick_output(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let dialog = rfd::FileDialog::new().add_filter("HTML plot", &["html"]);
            if let Some(path) = dialog.save_file() {
                window.set_plot_recording_output_path(path.to_string_lossy().into_owned().into());
            }
        });
    }

    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_plot_recording_run(move || {
            let Some(window) = window_weak.upgrade() else { return };
            run(&window, &state);
        });
    }

    {
        let window_weak = window.as_weak();
        window.on_plot_recording_open(move || {
            let Some(window) = window_weak.upgrade() else { return };
            let out_path = window.get_plot_recording_last_output().to_string();
            if !out_path.is_empty() {
                let _ = std::process::Command::new("explorer").arg(out_path).spawn();
            }
        });
    }
}

fn run(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    let Some(input) = state.borrow().plot_recording_input.clone() else {
        window.set_plot_recording_status("Error: pick a physics .rkyv file first.".into());
        return;
    };

    let output = window.get_plot_recording_output_path().to_string();

    let binary = process::resolve_binary("acr_plot_recording.exe");
    let input_str = input.to_string_lossy().into_owned();

    // Same "input dir if left blank" default `acr_plot_recording` itself
    // uses — computed here too so "Open plot" has something to point at
    // without needing to parse the tool's stdout for the path it chose.
    let resolved_output = if output.trim().is_empty() {
        let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        input
            .parent()
            .map(|p| p.join(format!("{stem}_plot.html")))
            .unwrap_or_else(|| PathBuf::from(format!("{stem}_plot.html")))
            .to_string_lossy()
            .into_owned()
    } else {
        output.clone()
    };

    window.set_plot_recording_running(true);
    window.set_plot_recording_status("Running…".into());
    window.set_plot_recording_log("".into());
    window.set_plot_recording_last_output("".into());

    let window_weak = window.as_weak();
    std::thread::spawn(move || {
        run_child(window_weak, binary, input_str, output, resolved_output)
    });
}

fn run_child(
    window_weak: Weak<AppWindow>,
    binary: PathBuf,
    input: String,
    output_arg: String,
    resolved_output: String,
) {
    let mut args: Vec<&str> = vec![input.as_str()];
    if !output_arg.trim().is_empty() {
        args.push(output_arg.as_str());
    }

    let result = process::run_and_wait(&binary, &args, |_is_stderr, line| {
        let window_weak = window_weak.clone();
        let line = line.to_string();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window_weak.upgrade() {
                append_log(&window, &line);
            }
        });
    });

    match result {
        Ok(r) if r.succeeded() => finish(&window_weak, true, "Done.".to_string(), resolved_output),
        Ok(r) => finish(&window_weak, false, r.failure_message("acr_plot_recording"), String::new()),
        Err(e) => {
            let msg = format!("Failed to launch {}: {e}", binary.display());
            finish(&window_weak, false, msg, String::new());
        }
    }
}

fn finish(window_weak: &Weak<AppWindow>, success: bool, status: String, output_path: String) {
    let window_weak = window_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window_weak.upgrade() {
            window.set_plot_recording_running(false);
            window.set_plot_recording_status(
                if success { status.into() } else { format!("Error: {status}").into() },
            );
            if success {
                window.set_plot_recording_last_output(output_path.into());
            }
        }
    });
}

fn append_log(window: &AppWindow, line: &str) {
    crate::append_line!(window, get_plot_recording_log, set_plot_recording_log, line);
}
