CREATE TABLE IF NOT EXISTS signed_price_feeds (
    id              BIGINT PRIMARY KEY CHECK (id >= 0),
    feed_type       TEXT        NOT NULL,
    description     TEXT        NOT NULL,
    price           BIGINT      NOT NULL CHECK (price >= 0),
    timestamp       BIGINT      NOT NULL CHECK (timestamp >= 0),
    valid_until     BIGINT      NOT NULL CHECK (valid_until >= 0),
    signature       BYTEA       NOT NULL
);
