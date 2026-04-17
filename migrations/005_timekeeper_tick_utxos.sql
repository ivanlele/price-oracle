-- Track issued timestamp tick UTXOs for return-to-issuer logic.
-- Each tick produces a 1-sat output that should be swept back
-- to the covenant supply if unused after the configured interval.
CREATE TABLE IF NOT EXISTS timekeeper_tick_utxos (
    id          SERIAL PRIMARY KEY,
    txid        TEXT        NOT NULL,
    vout        INTEGER     NOT NULL,
    amount      BIGINT      NOT NULL CHECK (amount > 0),
    created_at  BIGINT      NOT NULL CHECK (created_at >= 0),
    spent       BOOLEAN     NOT NULL DEFAULT FALSE,
    UNIQUE(txid, vout)
);
