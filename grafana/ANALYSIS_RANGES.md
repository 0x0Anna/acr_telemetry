# Analysis segments from Grafana annotations

Workflow: tag annotations in Grafana with `rid_<recording_id>` → call `acr_analysis_export --serve` via link → tool writes **analysis.db** (recordings, statics, graphics, analysis table with physics sliced to annotation time ranges).

## Steps

1. Start **acr_analysis_export --serve** once (in the background).
2. **In Grafana** (single-recording dashboard): create annotations (Ctrl+drag) and set tag `rid_55` (for recording 55).
3. **Dashboard link** (button): `http://localhost:9876/export?recording_id=${recording_id}`
4. Click the link → tool reads `grafana.db`, backs up to `analysis.db.bak`, writes **analysis.db**.

## acr_analysis_export

Writes **analysis.db** (same directory as `telemetry.db`, or `--analysis-db PATH`). Before overwriting, `analysis.db` is copied to `analysis.db.bak`.

**Server mode (for Grafana links):**

```
acr_analysis_export --serve [--port 9876] [--grafana-db PATH] [--telemetry-db PATH] [--analysis-db PATH]
```

**CLI mode:**

```
acr_analysis_export <recording_id> [--grafana-db PATH] [--telemetry-db PATH] [--analysis-db PATH]
```

Paths: `--grafana-db` (or `GRAFANA_DB`), `--telemetry-db` (or `acr_recorder.toml`), `--analysis-db` (default: directory of `telemetry.db` + `analysis.db`).

## Contents of analysis.db

- **recordings**: rows for the recording_ids used (column `id`)
- **statics**: rows with matching `recording_id`
- **graphics**: sliced to annotation time ranges
- **analysis**: sliced physics + `annotation_id`

## Grafana dashboard link

1. Edit dashboard → **Dashboard settings** (gear) → **Links** → **New link**
2. **Title**: e.g. `Export to analysis`
3. **URL**: `http://localhost:9876/export?recording_id=${recording_id}`
4. Enable **Open in new tab**
5. Save

The `recording_id` variable must exist on the dashboard (e.g. recording dropdown). **AC Rally full** already includes this link.
