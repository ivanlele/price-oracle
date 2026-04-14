CREATE TABLE IF NOT EXISTS price_feed_listings (
    id              BIGINT PRIMARY KEY CHECK (id >= 0),
    description     TEXT        NOT NULL
);
