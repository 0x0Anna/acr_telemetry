use rusqlite::Connection;

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS reference_runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    reference_track TEXT NOT NULL,
    car             TEXT NOT NULL,
    stage_slug      TEXT NOT NULL,
    sector_index    INTEGER NOT NULL,
    tot_sec         REAL NOT NULL,
    is_complete     INTEGER NOT NULL DEFAULT 1,
    invalidated     INTEGER NOT NULL DEFAULT 0,
    created_utc     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_reference_runs_lookup
    ON reference_runs (reference_track, car, stage_slug, sector_index, is_complete, invalidated, tot_sec);

CREATE TABLE IF NOT EXISTS reference_sub_splits (
    run_id    INTEGER NOT NULL REFERENCES reference_runs(id) ON DELETE CASCADE,
    sub_id    INTEGER NOT NULL,
    time_sec  REAL NOT NULL,
    PRIMARY KEY (run_id, sub_id)
);

CREATE TABLE IF NOT EXISTS sector_runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    reference_track TEXT NOT NULL,
    car             TEXT NOT NULL,
    stage_slug      TEXT NOT NULL,
    sector_index    INTEGER NOT NULL,
    tot_sec         REAL NOT NULL,
    cum_delta_sec   REAL NOT NULL,
    is_complete     INTEGER NOT NULL,
    invalidated     INTEGER NOT NULL DEFAULT 0,
    created_utc     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sector_run_subs (
    run_id    INTEGER NOT NULL REFERENCES sector_runs(id) ON DELETE CASCADE,
    sub_id    INTEGER NOT NULL,
    time_sec  REAL,
    delta_i   REAL,
    PRIMARY KEY (run_id, sub_id)
);
"#,
    )
}
