# ACR timing protocol (schema v1)

Events are published on an in-process bus (`EventSender` / `EventReceiver`). The same JSON payloads can be sent over UDP later.

## Event types

| Event | When |
|-------|------|
| `route_identified` | Reference track + stage locked; ordered `sub_ids` |
| `timing_started` | Timer active (~1 m forward from start rest) |
| `sector_started` | Main sector entered; reference sub times frozen |
| `sub_split` | Silent CP crossed; `leg_time_sec`, optional `delta_i_sec`, `cum_delta_sec` |
| `sector_completed` | Main sector end with at least one sub |
| `sector_incomplete` | Main sector end with no subs |
| `run_finished` / `run_invalidated` | Run end or crash |

## Reference model

- Fastest **complete** sector run per `(reference_track, car, stage_slug, sector_index)` in `reference_runs.sqlite`
- Comparison by **`sub_id`**, not list index
- Missing subs: no Δ contribution; display `[--]`
- `cum_delta_sec` = sum of per-sub Δ vs reference for crossed subs only

## Display (presenter)

```
S1: +0.423 [0:19.34] [0:23.45] [--] [0:14.00] ref: 1:31.45 tot: 0:45.32
```

- `ref:` — fastest **complete** reference sector time (`reference_tot_sec`; full sector, not partial cum Δ).
- `tot:` — your elapsed time in the current (or just finished) sector.

Last completed sector line + live line for current sector (max 8 sub slots).

## Crates

- `acr_timing_protocol` — events + bus
- `acr_timing_store` — SQLite reference runs
- `acr_timing_engine` — `RunCoordinator`, sector sessions
- `acr_timing_presenter` — OSD lines + beeps from events
