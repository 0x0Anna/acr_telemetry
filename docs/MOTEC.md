# Generating MoTeC data with ACR

MoTeC LD files can be produced in two ways:

1. **Live (no rkyv):** `acr_recorder --motec` or the standalone **`acr_motec`** binary — physics is buffered in memory and written as `.ld` on stop.
2. **Post-export:** **acr_export** on existing `.rkyv` recordings — besides a MoTeC-style **CSV**, a **MoTeC LD** file (`.ld`) is always written.

The `.ld` file is what you typically open in **MoTeC i2**.

The LD export is currently a **minimal, working implementation** (validated in this project with MoTeC i2 and an RBR MoTeC v105 workspace). Not every channel from an arbitrary workspace is mapped yet; options and batch behaviour are described in **[EXPORT.md](EXPORT.md)**.

---

## 1. Live recording (direct to LD)

```powershell
acr_recorder --motec
# or
acr_motec
# optional output directory:
acr_motec --out C:\Telemetry
```

Writes `acr_motec_<timestamp>.ld` to **raw_output_dir** (or `--out`). Physics only; no `.rkyv` file. Stop with Ctrl+C or the stop file (same as **acr_recorder**).

---

## 2. Recording for later export

1. Start the game (ACC or AC Rally), run **acr_recorder**, then end the session (e.g. Ctrl+C or the stop file).
2. Files such as `…rkyv` appear in the configured **raw_output_dir** (see `acr_recorder.toml`, `[recorder]` section).
3. **Recommendation for LD:** keep `record_graphics = true` (default) so a matching **`*.graphics.rkyv`** sidecar exists—the export can then include extra channels in the LD.

---

## 3. Export to CSV + LD

Use **acr_export** from a release build (e.g. `target\release\acr_export.exe`). Configuration is optional: `acr_recorder.toml` with `[export]` and `raw_output_dir`—see `config-examples/`.

### Single recording

```powershell
acr_export "C:\path\to\recording.rkyv" --csv
```

Without `--csv`, behaviour is the same if the config sets `default_method = "csv"` or no config is found (default is CSV).

### All recordings from the raw directory (from config)

```powershell
acr_export --rawDir --csv
```

### All recordings in a folder

```powershell
acr_export "C:\path\telemetry_raw" --csv
```

---

## 4. Output files

Next to each `.rkyv` file you get:

| File | Contents |
|------|----------|
| `<stem>.csv` | Physics channels (MoTeC-style CSV) |
| `<stem>.ld` | **MoTeC LD** (for i2) |
| `<stem>.graphics.csv` | Only if `*.graphics.rkyv` was present |

In **batch** mode, a `.rkyv` is skipped if **`<stem>.csv`** already exists (see [EXPORT.md](EXPORT.md)).

---

## 5. Opening in MoTeC

1. Start **MoTeC i2**.
2. Open the generated **`.ld`** file (wording varies by version: Open, Import, etc.).
3. Workspace: testing used an **RBR MoTeC v105** workspace; other workspaces may expect different channel names.

---

## 6. SQLite instead of MoTeC?

**`--sqlite`** and **`--csv`** cannot be combined in one run. Use SQLite for Grafana; for MoTeC run **`--csv`** again in a **separate** step (includes `.ld`).

---

Further flags, paths, and notes export: **[EXPORT.md](EXPORT.md)**.
