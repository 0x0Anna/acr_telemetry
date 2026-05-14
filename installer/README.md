# ACR Recorder – Windows installer

The installer copies all binaries and configuration templates into the chosen directory. Paths in the TOML files are **relative to the install directory** (option-3 style behaviour).

## Prerequisites

1. **Rust** – release build:
   ```cmd
   cargo build --release
   ```
2. **Inno Setup 6** – [jrsoftware.org](https://jrsoftware.org/ishell.php) (free). During setup, enable “Inno Setup Preprocessor” (for future extensions).

## Building the installer

From the **project root**:

```cmd
"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\ACR_Recorder.iss
```

Or open Inno Setup, load `installer\ACR_Recorder.iss`, then **Build → Compile** (F9).

The generated setup ends up in:

- `target\installer\ACR_Recorder_0.1.0_setup.exe`

## What the installer does

- Installs to `%LOCALAPPDATA%\Programs\ACR_Recorder` (or a directory you pick):
  - `acr_recorder.exe`, `acr_export.exe`, `acr_telemetry_bridge.exe`, `acr_analysis_export.exe`
  - `acr_recorder.toml` (only if missing yet, from template with relative paths)
  - `acr_telemetry_bridge.toml`, `telemetry_color.toml` (only if missing yet)
  - `batch\` with `acr_stop.bat`, `acr_marker_good.bat`, etc.
- Start menu entries: ACR Recorder, ACR Telemetry Bridge, Uninstall
- Optional: desktop and Quick Launch icons
- After install, optional: “Start ACR Recorder” / “Start Telemetry Bridge”

## Changing the version

Edit this line in `installer\ACR_Recorder.iss`:

```iss
#define MyAppVersion "0.1.0"
```

Optional: drive the version from `Cargo.toml` via a small script or the preprocessor.
