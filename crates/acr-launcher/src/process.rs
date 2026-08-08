//! Reusable child-process spawn/monitor helpers for launching one of
//! `acr_telemetry`'s CLI binaries (`acr_recorder.exe`, `acr_motec.exe`,
//! `acr_export.exe`, ...) without a console window popping up, and
//! streaming its stdout/stderr back into the GUI line-by-line.
//!
//! Deliberately generic — not specific to recording vs. exporting — so
//! every panel (recorder, export, track match, grip estimator, plot
//! recording, telemetry bridge) shares the same spawn/stream/wait
//! plumbing rather than re-implementing it. [`run_and_wait`]/
//! [`wait_for_output`] are the two entry points panels actually call;
//! [`spawn_hidden`]/[`stream_output`] are their building blocks, exposed
//! separately for the handful of panels (recorder, track match live,
//! telemetry bridge) that need to spawn synchronously on the UI thread
//! for immediate "failed to start" feedback before handing the child off
//! to a background thread.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;

/// Diagnostic logging that can't panic. Built with `windows_subsystem =
/// "windows"` (see `main.rs`), this app has no console attached, so
/// `eprintln!`'s `.expect("failed printing to stdout")` would abort the
/// process the first time a diagnostic line fires. Swallow the write
/// error instead — there's nowhere for the line to go, but that's not
/// worth crashing the GUI over.
pub fn log_err(msg: impl std::fmt::Display) {
    let _ = writeln!(std::io::stderr(), "{msg}");
}

/// Windows `CREATE_NO_WINDOW` process creation flag: suppresses the
/// console window that would otherwise flash up for a console
/// subprocess spawned from a GUI app. See
/// `CreateProcessW`/`STARTUPINFO` docs.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A line of output from a spawned child process, tagged by which stream
/// it came from so callers can decide how to display/filter each (e.g.
/// only stderr carries the recorder/exporter's progress lines today, but
/// both are forwarded so nothing is silently dropped).
#[derive(Debug, Clone)]
pub enum ChildOutput {
    Stdout(String),
    Stderr(String),
    /// The child process exited, with its exit code if one was available
    /// (a `None` exit status, e.g. terminated by a signal, has no
    /// portable equivalent on Windows — treated as `None` here too).
    Exited(Option<i32>),
}

/// The directory the launcher's own executable lives in — same
/// convention `acr_recorder::config::base_dir()` uses, so a binary
/// dropped next to `acr-launcher.exe` resolves the same way the
/// existing batch-file workflow expects (README already tells users to
/// keep all `acr_*.exe` together).
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Resolve `binary_name` (e.g. `"acr_recorder.exe"`) against [`exe_dir`].
/// Falls back to the bare name (resolved via `PATH`) if the launcher's
/// own directory can't be determined.
pub fn resolve_binary(binary_name: &str) -> PathBuf {
    exe_dir()
        .map(|dir| dir.join(binary_name))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(binary_name))
}

/// Spawn `binary_path` with `args`, piping stdout/stderr and suppressing
/// the console window it would otherwise create. Callers own the
/// returned [`Child`] (for e.g. checking `try_wait()` or holding it
/// alive) — pair with [`stream_output`] to read its stdout/stderr on a
/// background thread.
pub fn spawn_hidden(
    binary_path: &Path,
    args: &[&str],
) -> std::io::Result<Child> {
    Command::new(binary_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
}

/// Take `child`'s stdout and stderr (if present) and forward each line —
/// plus a final [`ChildOutput::Exited`] once the process ends — to
/// `sender` from a single background thread. Silently does nothing with
/// lines once the receiving end is dropped (a closed channel just ends
/// the thread early via the `let _ =` below rather than panicking).
///
/// Blocks the *spawned* thread, not the caller: this function returns
/// immediately, leaving the reading/waiting to run in the background.
pub fn stream_output(mut child: Child, sender: Sender<ChildOutput>) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    std::thread::spawn(move || {
        // Read stderr on its own thread (recorder/export binaries print
        // their progress there — see src/main.rs's eprintln! calls, e.g.
        // "Recording to: ...", "Waiting for ACC shared memory...",
        // "Done. Recorded N samples...") while stdout is read inline
        // below, so neither pipe can back up and stall the child.
        let stderr_sender = sender.clone();
        let stderr_thread = stderr.map(|stderr| {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if stderr_sender.send(ChildOutput::Stderr(line)).is_err() {
                        return;
                    }
                }
            })
        });

        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(ChildOutput::Stdout(line)).is_err() {
                    break;
                }
            }
        }

        if let Some(t) = stderr_thread {
            let _ = t.join();
        }

        let exit_code = child.wait().ok().and_then(|status| status.code());
        let _ = sender.send(ChildOutput::Exited(exit_code));
    });
}

/// Whether a process named `image_name` (e.g. `"acr.exe"`) currently
/// appears in a process snapshot. Used for the Status tab's "Launch AC
/// Rally" button instead of the ACC-shared-memory poll — overlay tools
/// like SimHub keep the ACC-shaped shared memory segments
/// (`Local\acpmf_physics` etc.) alive on their own, so shared-memory
/// presence alone doesn't mean the game's own process is actually up.
///
/// Walks a `CreateToolhelp32Snapshot` process list in-process rather than
/// spawning `tasklist.exe` — this runs once a second for the launcher's
/// whole lifetime (see `main.rs`'s `spawn_status_poll`), and shelling out
/// to a fresh process every second is unnecessary process-creation
/// overhead for what's a simple name lookup.
pub fn is_process_running(image_name: &str) -> bool {
    use std::ffi::CStr;
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: `CreateToolhelp32Snapshot`/`Process32First`/`Process32Next`
    // are called per their documented contract — the snapshot handle is
    // checked against `INVALID_HANDLE_VALUE` before use and always closed
    // on every return path, and `entry` is a plain `#[repr(C)]` struct
    // zero-initialized before being handed to the Win32 calls that fill it in.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }

        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;

        let mut found = false;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                let name = CStr::from_ptr(entry.szExeFile.as_ptr())
                    .to_string_lossy();
                if name.eq_ignore_ascii_case(image_name) {
                    found = true;
                    break;
                }
                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        found
    }
}

/// The outcome of a child process run to completion via [`wait_for_output`]
/// / [`run_and_wait`]: its exit code (if any) and the last output line
/// seen on either stream, for building the "X exited with code N. Last
/// output: ..." messages every fire-and-forget panel used to hand-roll.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub exit_code: Option<i32>,
    pub last_line: String,
}

impl RunResult {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// `"{program} exited with code N. Last output: ..."`, or "exited
    /// abnormally" if no exit code was available (see [`ChildOutput::Exited`]).
    pub fn failure_message(&self, program: &str) -> String {
        match self.exit_code {
            Some(code) => format!(
                "{program} exited with code {code}. Last output: {}",
                self.last_line
            ),
            None => format!("{program} exited abnormally. Last output: {}", self.last_line),
        }
    }
}

/// Stream `child`'s output to completion on the calling thread, invoking
/// `on_line(is_stderr, line)` for each line as it arrives and returning
/// once the process exits. Blocks the caller — every current caller
/// already runs this from its own background thread (see
/// `recorder_panel::start_recording` for the "spawn synchronously on the
/// UI thread, then hand the child to a background thread" shape this is
/// meant for).
pub fn wait_for_output(child: Child, mut on_line: impl FnMut(bool, &str)) -> RunResult {
    let (tx, rx) = std::sync::mpsc::channel();
    stream_output(child, tx);

    let mut exit_code = None;
    let mut last_line = String::new();
    for msg in rx {
        match msg {
            ChildOutput::Stdout(line) => {
                last_line = line.clone();
                on_line(false, &line);
            }
            ChildOutput::Stderr(line) => {
                last_line = line.clone();
                on_line(true, &line);
            }
            ChildOutput::Exited(code) => exit_code = code,
        }
    }
    RunResult { exit_code, last_line }
}

/// [`spawn_hidden`] + [`wait_for_output`] in one call, for the
/// fire-and-forget panels (export, track match offline, grip estimator,
/// plot recording) that spawn from inside a background thread already and
/// so don't need the two steps split apart for synchronous "failed to
/// launch" feedback.
pub fn run_and_wait(
    binary: &Path,
    args: &[&str],
    on_line: impl FnMut(bool, &str),
) -> std::io::Result<RunResult> {
    let child = spawn_hidden(binary, args)?;
    Ok(wait_for_output(child, on_line))
}

/// Append `$line` to a Slint text property, prefixing it with a newline
/// unless the property is currently empty — the "get, push newline if
/// nonempty, push line, set" dance every panel's log/results property
/// needs. A macro rather than a function since Slint's generated
/// getters/setters (`get_export_log`/`set_export_log`, etc.) are distinct
/// methods per property with no common trait to write one function against.
#[macro_export]
macro_rules! append_line {
    ($window:expr, $get:ident, $set:ident, $line:expr) => {{
        let mut text = $window.$get().to_string();
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str($line);
        $window.$set(text.into());
    }};
}
