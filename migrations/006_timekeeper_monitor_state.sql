CREATE TABLE IF NOT EXISTS timekeeper_monitor_state (
    id                  INTEGER PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    last_scanned_height BIGINT NOT NULL CHECK (last_scanned_height >= 0),
    last_scanned_hash   TEXT   NOT NULL,
    updated_at          BIGINT NOT NULL CHECK (updated_at >= 0)
);