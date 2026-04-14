# Price Oracle

The Price Oracle Service is a backend system designed to fetch, sign, and serve asset price data. It crawls on-chain Chainlink-compatible price feed contracts, signs the results with a secp256k1 Schnorr key, and exposes them over a REST API.

## Quick Start

### 1. Configuration

Copy `config.example.toml` to `config.toml` and edit it as needed. For demonstration, the service is almost ready with this configuration, you only need to provide [service.feed_crawler.rpc_url].

### 3. Run the Service

```bash
cargo run -- start # uses config.toml by default
cargo run -- start --config my.toml # use a custom config path
```

The service will:

1. Run database migrations automatically
2. Initialize the feed crawler
3. Perform an initial crawl of all configured feeds
4. Start the HTTP server and schedule recurring crawls at the configured interval

## API Documentation (Swagger)

A separate binary serves the OpenAPI spec with Swagger UI:

```bash
cargo run --bin doc
```

This starts a Swagger UI at `http://localhost:8081/swagger-ui/` where you can browse and try out all endpoints.

## Crawler

The crawler periodically reads prices from on-chain Chainlink-compatible `EACAggregatorProxy` contracts. On startup it queries each contract's `description()` (e.g. `"BTC / USD"`) and `decimals()` to build a lookup table, then matches the configured `feeds` entries against this table.

Each crawled feed is signed with a Schnorr signature using the configured private key. The signed result is stored in PostgreSQL and served via the API.

### Feed Types

There are two types of feeds, determined by the format of entries in the `feeds` config array:

#### Exchange Rate (simple pair)

A two-part path like `"BTC/USD"` is an exchange rate feed. The crawler finds a single contract whose on-chain description matches the pair (in either direction) and returns its `latestRoundData` answer directly.

#### Cross Rate (derived pair)

A path with three or more parts like `"BTC/USD/USDT"` is a **cross rate** feed. The crawler chains multiple contracts to derive a rate that no single contract provides. Each consecutive pair of currencies in the path is resolved to a contract:

For example, `BTC/USD/USDT` with contracts:

- Contract 0: `BTC / USD` (8 decimals)
- Contract 1: `USDT / USD` (8 decimals)

Step 1: BTC -> USD — uses contract 0 directly (multiply)
Step 2: USD -> USDT — uses contract 1 inverted (divide), because the contract gives USDT/USD, not USD/USDT

The converter handles the fixed-point arithmetic and decimal scaling automatically.

### Adding a New Pair

1. Deploy or identify the `EACAggregatorProxy` contract address for the pair on your target chain.
2. Add the contract address to `service.feed_crawler.addresses` in `config.toml`.
3. Add the feed to `service.feed_crawler.feeds`:
   - For a direct pair: `"ETH/USD"`
   - For a derived cross rate: `"ETH/USD/USDT"` (each hop must be covered by a contract in `addresses`)
4. Restart the service. The new feed will be auto-registered and crawled on the next cycle.

## License

This project is licensed under the MIT License. See the [LICENCE](LICENCE) file for details.
