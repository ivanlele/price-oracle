--- Track the timestamp asset ID and issuance transaction.
CREATE TABLE IF NOT EXISTS timekeeper_assets (
    id              SERIAL PRIMARY KEY,
    asset_id        TEXT        NOT NULL UNIQUE,
    issuance_txid   TEXT        NOT NULL,
    contract_hash   TEXT        NOT NULL,
    created_at      BIGINT      NOT NULL CHECK (created_at >= 0)
);
