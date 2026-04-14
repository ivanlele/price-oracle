CREATE TABLE IF NOT EXISTS signed_price_feeds (
    id              INTEGER PRIMARY KEY,
    feed_type       TEXT        NOT NULL,
    description     TEXT        NOT NULL,
    price           BIGINT      NOT NULL,
    timestamp       INTEGER     NOT NULL,
    valid_until     INTEGER     NOT NULL,
    signature       BYTEA       NOT NULL
);
