-- Track the current covenant UTXO for the timestamp asset.
CREATE TABLE IF NOT EXISTS timekeeper_supply_utxos (
    id          SERIAL PRIMARY KEY,
    txid        TEXT        NOT NULL,
    vout        INTEGER     NOT NULL,
    amount      BIGINT      NOT NULL CHECK (amount > 0),
    created_at  BIGINT      NOT NULL CHECK (created_at >= 0),
    spent       BOOLEAN     NOT NULL DEFAULT FALSE,
    UNIQUE(txid, vout)
);
