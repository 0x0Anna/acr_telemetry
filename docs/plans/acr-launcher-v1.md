# Slint launcher for acr_telemetry — v1 (status, record, export)

Planning artifact from a Claude Code session, kept in-repo so future sessions/subagents
can pick up implementation (including in parallel — see "Parallelizable work units" at
the end) without re-deriving this context. Original plan-mode file:
`C:\Users\annag\.claude\plans\expressive-knitting-raccoon.md` (local to the machine that
planned this, not guaranteed to persist — this copy is the durable one).

## Context

`acr_telemetry` has grown into ~18 separate CLI binaries (recorder, export, motec,
timing, track_match, grip_estimator, bridge, plot_recording, …) driven from batch files
and a shell. There's no unified front end — `acr_receiver/` is a static live-telemetry
dashboard, not a launcher. Decided to build a desktop launcher the same way the sibling
`shakedown-engineer` repo builds `sde-app`: a Slint GUI crate in this workspace that
shells out to the existing binaries rather than reimplementing their logic.

First pass covers, in priority order: (1) graceful handling of "ACC/AC Rally not
running" instead of the recorder crashing, (2) start/stop recording with the key config
options exposed, (3) exporting recordings (CSV/SQLite/LD). MoTeC-writer channel-coverage
improvements and the rest of the tool surface (timing, track_match, bridge, …) are
scoped as backlog, not built in this pass.

## Root-cause bug found: `acr_recorder` crashes when the game isn't running yet

`src/main.rs`'s normal (non-`--motec`) path does `let mut acc = ACCSharedMemory::new()?;`
(`src/main.rs:139`) — if ACC/AC Rally hasn't created its shared-memory segments yet,
`SharedMemoryReader::new` (`vendor/acc_shared_memory_rs/src/core/shared_memory.rs:42-59`)
returns `ACCError::SharedMemoryNotAvailable`, which propagates straight out of `main`
and exits with a raw `Debug`-printed error.

The `--motec` / `acr_motec` path already handles this correctly: `motec_live.rs`'s
`open_acc_or_wait` (`src/motec_live.rs:188-216`) polls `ACCSharedMemory::new()` every
500ms, printing "Waiting for ACC shared memory…" every 5s, until it connects or a
stop/ctrl-c is requested.

**Fix:** extract `open_acc_or_wait` into a small shared helper (e.g. `src/acc_wait.rs`,
`pub fn open_or_wait(running: &AtomicBool, stop_path: &Path) -> Result<Option<ACCSharedMemory>,
Box<dyn Error>>`) and use it from both `motec_live::run` and `main.rs`'s rkyv-recording
path (replacing the bare `ACCSharedMemory::new()?` at `main.rs:139`). This is a backend
fix independent of the GUI, and it's what makes "launcher starts a recording before the
game is up" a non-event instead of a crash.

## New crate: `crates/acr-launcher`

New workspace member (add to root `Cargo.toml`'s `members`), a Slint binary following
`sde-app`'s established conventions (`shakedown-engineer/crates/sde-app/src/main.rs`):
`Rc<RefCell<AppState>>` shared across callbacks, `window.as_weak()` + `upgrade()` inside
each closure, one `window.on_xxx(move |...| {...})` block per action, a `config_dir()`
helper under `%APPDATA%`.

```
crates/acr-launcher/
  Cargo.toml
  build.rs                 # slint_build::compile, mirrors sde-app/build.rs
  ui/app.slint              # window: status bar + 3 tabs (Status, Record, Export)
  src/main.rs                # AppState, callbacks, window.run()
  src/process.rs             # child-process spawn/monitor helpers
  src/game_status.rs          # background ACC/AC Rally shared-memory poll
  src/recorder_panel.rs       # config load/edit/save, start/stop recording
  src/export_panel.rs         # export invocation + output log
```

Dependencies (`crates/acr-launcher/Cargo.toml`):
- `acr_recorder = { path = "../.." }` — reuse the existing lib crate directly instead of
  re-deriving config parsing: `config::{Config, load_config, resolve_path,
  resolve_stop_file_path, resolve_notes_dir}` are already `pub`.
- `acc_shared_memory_rs = { path = "../../vendor/acc_shared_memory_rs" }` — for the
  launcher's own independent "is the game running" poll.
- `slint = "1.17"`, `rfd = "0.17"` (file/folder pickers) — same versions as `sde-app`.
- `slint-build = "1.17"` as a build-dependency.
- No new dependency needed for TOML editing in v1 (see "Recording panel" below).

## Game-running status (independent of any active recording)

`src/game_status.rs`: a background thread owns its own `ACCSharedMemory` attempt,
polling `ACCSharedMemory::new()` (cheap — just tries to open the named shared-memory
segments) once a second, and posts Connected/NotRunning transitions back to the UI
thread via `slint::invoke_from_event_loop` + `window.as_weak()`. This is deliberately
separate from whatever `acr_recorder`/`acr_motec` subprocess may or may not be running,
so the Status tab shows "● ACC/AC Rally not detected" the moment the launcher opens,
before the user has clicked anything. The Record tab reuses the same signal to show a
"waiting for game…" hint instead of a bare disabled button — recording can still be
*started* while the game is down (the backend fix above makes that safe to do; the
subprocess will sit in its own wait loop until the game appears).

## Recording panel

- **Config options exposed**: `raw_output_dir`, `notes_dir`, `record_graphics` toggle,
  `ring_mode` (+ `ring_slots`, `ring_prefix`), and a "Record directly to MoTeC (no rkyv)"
  toggle that switches which binary gets spawned (`acr_motec.exe` instead of
  `acr_recorder.exe`) — mirrors the existing `--motec` flag / `acr_motec` binary duality.
- **Config load/save**: read `acr_recorder.toml` via `acr_recorder::config::load_config`
  at startup to pre-fill the form. On "Save", write the edited subset back out with
  `toml::to_string_pretty` over the full `Config` struct (already `serde::Deserialize`;
  add `Serialize` derives where missing). **Known trade-off**: this round-trip does not
  preserve comments/formatting in an existing hand-edited `acr_recorder.toml` — acceptable
  for v1 since the file is small and the GUI becomes the primary editor going forward;
  flagged as a backlog item (switch to `toml_edit` for comment-preserving in-place edits)
  if that turns out to matter in practice.
- **Start**: spawn the chosen binary (resolved next to the launcher's own `exe_dir`,
  matching how the README already tells users to keep all `acr_*.exe` together) as a
  child process with piped stdout/stderr, `CREATE_NO_WINDOW` (via
  `std::os::windows::process::CommandExt::creation_flags`) so no console flashes up.
  A reader thread tails stderr (where the recorder prints its progress/status lines)
  and forwards lines into the UI: a small set of known substrings — "Waiting for ACC
  shared memory", "Connected to ACC shared memory", "Recording to:", "Done. Recorded"
  — drive the status pill; everything else goes into a scrolling raw-output log panel
  for visibility/debugging. (Backlog: add a `--status-json` flag to `acr_recorder`/
  `acr_motec` for a real structured status channel instead of substring matching —
  not needed for v1, current text output is stable enough.)
- **Stop**: do **not** kill the process. Write the stop file at
  `acr_recorder::config::resolve_stop_file_path(&cfg.recorder)`, exactly like
  `acr_stop.bat` does, so the recorder flushes and exits on its own. The UI shows
  "Stopping…" until the reader thread sees the process exit.

## Export panel

- Pick a `.rkyv` file or the configured `raw_output_dir` (via `rfd`, defaulting to
  `config.recorder.raw_output_dir`), or a "batch: whole raw dir" mode (`--rawDir`,
  matching `acr_export`'s existing CLI contract in `src/bin/acr_export.rs`'s header doc
  comment).
- Method checkboxes: CSV, SQLite (with a path field defaulting to
  `config.export.sqlite_db_path`); LD is always written by `acr_export` regardless
  (per `docs/MOTEC.md` §4), so no separate toggle needed for it.
- Spawn `acr_export.exe` the same way as the recorder (piped output, no console
  window), stream its stdout/stderr into a log panel, and surface exit status
  (success / failed with the last error line highlighted).
- On success, a "Open output folder" button (`explorer.exe /select,<path>` or just
  opening the directory) — small but removes the last manual step.

## Explicitly out of scope for this pass (backlog)

- **MoTeC writer channel-coverage improvements**: `docs/MOTEC.md` already flags the LD
  export as "minimal … not all workspace channels mapped yet." Needs input on which
  channels/workspace to prioritize; ties into the not-yet-validated `sde-formats/acr`
  CSV parser in `shakedown-engineer` — revisit once that validation is done.
- **Other tool tabs**: `acr_timing`, `acr_track_match`, `acr_telemetry_bridge`,
  `acr_grip_estimator`, `acr_plot_recording`, `acr_analysis_export` etc. are all
  candidates for further launcher tabs later. The crate structure (one
  `src/xxx_panel.rs` + one Slint tab per tool) is deliberately kept easy to extend.
- **Structured CLI status output** (`--status-json` or similar) instead of stderr
  substring matching.
- **Hotkey / HID (game controller, button box) bindings** for start/stop and other key
  actions — today this is only reachable via `batch/acr_stop.bat` etc. bound at the OS
  level. A future pass could add configurable bindings (keyboard global hotkeys and/or
  raw HID input from a button box) directly in the launcher so a wheel/button-box button
  can trigger Start/Stop/mark-good/mark-bad without a separate batch-file+OS-binding
  setup. Needs its own scoping (which HID crate, global-hotkey vs raw HID, conflict
  handling with the game capturing the same device).

## Files touched

**Backend fix**:
- `src/motec_live.rs`: make `open_acc_or_wait` reusable (move to new `src/acc_wait.rs`
  or make `pub` and `pub(crate)`-import from `main.rs`).
- `src/main.rs`: replace `ACCSharedMemory::new()?` (line 139) with the shared wait helper.
- `src/lib.rs`: register the new module if extracted to its own file.

**New launcher crate**:
- `Cargo.toml` (workspace root): add `crates/acr-launcher` to `members`.
- `crates/acr-launcher/{Cargo.toml,build.rs,ui/app.slint,src/*.rs}` as laid out above.

## Verification

1. `cargo build` with the game **not** running: `acr_recorder` should print "Waiting
   for ACC shared memory…" and stay alive instead of exiting — confirms the backend fix.
2. `cargo run -p acr-launcher`: Status tab shows "not detected" immediately; launch
   ACC/AC Rally and confirm it flips to "connected" within ~1s without restarting.
3. Start a recording from the Record tab before the game is up, then start the game —
   confirm the launcher's status pill transitions waiting → recording and a `.rkyv`
   file appears in the configured `raw_output_dir`.
4. Click Stop; confirm the stop file approach works (process exits on its own, "Done.
   Recorded N samples" line surfaces in the log) rather than being killed.
5. Run Export against that recording (CSV + SQLite); confirm `<stem>.csv`, `<stem>.ld`,
   and the SQLite DB update as `docs/EXPORT.md`/`docs/MOTEC.md` describe.
6. Edit a config option (e.g. toggle `record_graphics`) in the Record tab, save, and
   confirm the on-disk `acr_recorder.toml` reflects it and a subsequent recording
   respects it.

## Parallelizable work units

These can be picked up independently once the crate skeleton (unit A) exists — A must
land first since B/C/D all depend on `AppWindow` being generated from `ui/app.slint`
and the workspace member being wired up, but B, C, and D have no dependency on each
other and can proceed in parallel after that:

- **A. Backend fix + crate skeleton** (do first, blocks the rest): the
  `acc_wait`/`main.rs` fix, `Cargo.toml` workspace member addition, `crates/acr-launcher`
  scaffolding (`Cargo.toml`, `build.rs`, minimal `ui/app.slint` with the 3-tab shell and
  empty placeholder content, minimal `src/main.rs` that just opens the window).
- **B. Game status** (`src/game_status.rs` + its slint properties/tab): independent
  background-thread poll, only needs the window handle from A.
- **C. Recording panel** (`src/recorder_panel.rs`, `src/process.rs`'s spawn/monitor
  helpers, the Record tab's slint markup): needs `process.rs` helpers, which C should
  build (D can reuse them once C lands, or they can be stubbed/duplicated briefly and
  merged — process.rs is small enough this isn't a real blocker either way).
- **D. Export panel** (`src/export_panel.rs`, the Export tab's slint markup): same
  process-spawning shape as C; can share `process.rs` once it exists or implement its
  own minimal spawn helper if working fully in parallel with C.

Recommended split for subagents: one agent for A (sequential, must finish first), then
up to two more agents for {B} and {C+D combined, since they're both "spawn a process,
stream output" and share `process.rs`} — or three-way B/C/D in parallel if `process.rs`
is factored out as part of A's skeleton instead (cheap to do while A is already touching
`Cargo.toml`/module wiring).
