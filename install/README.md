# ACR Recorder – Windows install package



Builds a **portable zip** and an **Inno Setup 6** installer (`setup.exe`). The user picks the installation directory in the setup wizard; all paths in the shipped TOML files are **relative to that folder**.



## Prerequisites



1. **Rust** – [rust-lang.org](https://www.rust-lang.org/tools/install/) (MSVC build tools on Windows).

2. **Inno Setup 6** (optional, for `setup.exe`) – [jrsoftware.org](https://jrsoftware.org/ishell.php).



## Build



From the **repository root**:



```powershell

pwsh install\build.ps1

```



Outputs:



| Artifact | Path |

|----------|------|

| Staged payload | `install\staging\` (gitignored) |

| Portable zip | `target\install\ACR_Recorder_<version>_windows-x64_portable.zip` |

| Installer | `target\install\ACR_Recorder_<version>_setup.exe` |



CI: `pwsh install/build.ps1 -SkipCargoBuild` after `cargo build` (see `.github/workflows/release.yml`).



## What gets installed



**Programs** (start menu shortcuts use `{app}` as working directory):



- `acr_recorder`, `acr_export`, `acr_motec`, `acr_telemetry_bridge`

- `acr_analysis_export`, `acr_track_match`, `acr_timing`, `acr_rtss_osd`, `acr_analyze_timing_recording`



**Configuration** (copied on first install; not overwritten on upgrade):



- `acr_recorder.toml`, `acr_motec.toml`, `acr_timing.toml`, `acr_track_match.toml`

- `acr_telemetry_bridge.toml`, `telemetry_color.toml`



Installer-ready templates: `install\config\` (relative paths). Reference copies with comments: `config-examples\` (bundled in zip/setup; also `acr-<tag>-config-examples.zip` on Releases). All `docs\*.md` are included in `docs\` and as `acr-<tag>-docs.zip`.



**Data layout** (created by installer + bundled assets):



```

<install-dir>/

  *.exe

  *.toml

  batch/

  docs/

  telemetry_raw/       empty (recordings)

  timing/              sectors_filtered.shp, timing_sectors/, cumulative_sectors/

  timing/runs/         HTML reports at runtime

  assets/split_sounds/ WAV split feedback ([cumulative_beep])

  voices/timing_en/    optional blame/copilot clips ([timing_voice])

  reference_tracks/    Bundled shapefiles (hafren_north, saverne, …)

```



Default install path: `C:\tools\acr_telemetry` (changeable in wizard). Creating `C:\tools` may require administrator rights on first install.



Notes/stop files still use `%APPDATA%\acr_telemetry` unless you set `notes_dir` in `acr_recorder.toml`.

**External timing provider / game-clock mod:** not included in the installer (no `main.lua`, no optional task). Gate timing and MoTeC recording work without it. Optional JSONL integration is configured via `[game_clock]` in `acr_timing.toml` when you supply your own provider.

## Layout in this folder



| Path | Role |

|------|------|

| `config\` | Installer TOML templates (relative paths) |

| `assets\` | README placeholders for empty dirs |

| `ACR_Recorder.iss` | Inno Setup script |

| `build.ps1` | Stage + zip + ISCC |

| `PACKAGE_README.txt` | Shipped as `README.txt` in install dir |



## Version



Bump `version` in root `Cargo.toml` (semver `X.Y.Z` only). CI derives the installer version from the git tag (`v0.0.8.1` → `0.0.8.1`, `v0.0.8_timing` → `0.0.8_timing`). Local override: `install/build.ps1 -Version 0.0.8.1`.

