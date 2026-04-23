use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::str::FromStr;
use std::{io, io::Write};

use clap::{ArgAction, Parser};
use electrsd::bitcoind::bitcoincore_rpc::{Auth, Client as RpcClient, RpcApi};
use price_oracle::config::Config;
use price_oracle::elements;
use price_oracle::elements::{OutPoint, Txid};
use price_oracle::timekeeper::script_hash;
use price_oracle::{
    artifacts::oracle_price_guard::OraclePriceGuardProgram,
    artifacts::oracle_price_guard::derived_oracle_price_guard::{
        OraclePriceGuardArguments, OraclePriceGuardWitness,
    },
    artifacts::timestamp_covenant::TimestampCovenantProgram,
    artifacts::timestamp_covenant::derived_timestamp_covenant::{
        TimestampCovenantArguments, TimestampCovenantWitness,
    },
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use simplex::provider::ProviderTrait;
use simplex::signer::Signer;
use simplex::transaction::{
    FinalTransaction, PartialInput, PartialOutput, RequiredSignature, partial_input::ProgramInput,
    utxo::UTXO,
};

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_MAGENTA: &str = "\x1b[35m";
const ANSI_RED: &str = "\x1b[31m";

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = "config.toml")]
    config: PathBuf,
    #[arg(long, default_value_t = 1)]
    feed_id: u32,
    #[arg(long)]
    threshold: u64,
    #[arg(long, default_value_t = 50_000)]
    fund_amount: u64,
    #[arg(long, default_value_t = 1_000)]
    fee_reserve: u64,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    interactive: bool,
}

#[derive(Debug, Deserialize)]
struct PublicKeyResponse {
    public_key: String,
}

#[derive(Debug, Deserialize)]
struct IssuerSpkResponse {
    issuer_spk: String,
}

#[allow(unused)]
#[derive(Debug, Deserialize)]
struct TickItem {
    txid: String,
    vout: i32,
    amount: i64,
    created_at: i64,
}

#[derive(Debug, Deserialize)]
struct TickListResponse {
    items: Vec<TickItem>,
    total: i64,
}

#[derive(Debug, Deserialize)]
struct FeedResponse {
    id: u32,
    price: u64,
    timestamp: u32,
    valid_until: u32,
    signature: String,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();

    let config = Config::from_file(args.config).map_err(|e| e.to_string())?;
    let signer = config.service.timekeeper.signer();
    let provider = signer.get_provider();
    let network = *provider.get_network();
    let base_url = format!("http://127.0.0.1:{}", config.service.port);
    let client = Client::new();
    let rpc = new_timekeeper_rpc(&config)?;

    let pubkey: PublicKeyResponse =
        get_json(&client, &format!("{base_url}/price-oracle/public-key")).await?;
    let issuer: IssuerSpkResponse = get_json(
        &client,
        &format!("{base_url}/price-oracle/timekeeper/issuer-spk"),
    )
    .await?;
    let feed: FeedResponse = get_json(
        &client,
        &format!("{base_url}/price-oracle/feed/{}", args.feed_id),
    )
    .await?;

    let mut tried_tick_outpoints = HashSet::new();
    let initial_tick = select_unspent_tick(&client, &base_url, &rpc, &tried_tick_outpoints).await?;
    let initial_tick_txid = elements::Txid::from_str(&initial_tick.txid)
        .map_err(|e| format!("invalid tick txid: {e}"))?;
    let initial_tick_utxo =
        fetch_utxo_by_outpoint(provider, &initial_tick_txid, initial_tick.vout as u32)?;
    let guard_tick_asset_id = initial_tick_utxo
        .explicit_asset()
        .into_inner()
        .to_byte_array();

    let oracle_pubkey = elements::secp256k1_zkp::PublicKey::from_str(&pubkey.public_key)
        .map_err(|e| format!("invalid oracle public key: {e}"))?;
    let (oracle_xonly, _) = oracle_pubkey.x_only_public_key();

    let issuer_spk_bytes =
        hex::decode(&issuer.issuer_spk).map_err(|e| format!("invalid issuer script hex: {e}"))?;
    let issuer_spk = elements::Script::from(issuer_spk_bytes);
    let issuer_script_hash = script_hash(&issuer_spk);

    let claim_address = signer.get_address();
    let claim_spk = claim_address.script_pubkey();
    let claim_script_hash = script_hash(&claim_spk);

    let guard_spk = OraclePriceGuardProgram::new(OraclePriceGuardArguments {
        oracle_pubkey: oracle_xonly.serialize(),
        feed_id: args.feed_id,
        max_price: args.threshold,
        claim_script_hash,
        tick_asset_id: guard_tick_asset_id,
    })
    .get_program()
    .get_script_pubkey(&network);

    let funding_txid = fund_guard(&signer, &guard_spk, args.fund_amount)?;
    print_status_block(
        "Oracle Guard Funded",
        ANSI_GREEN,
        &[
            ("Funding Tx", funding_txid.to_string()),
            ("Guard Script", hex::encode(guard_spk.as_bytes())),
        ],
    );

    if args.interactive {
        interactive_tx_checkpoint(&rpc, &funding_txid, "funding transaction")?;
    }

    provider
        .wait(&funding_txid)
        .map_err(|e| format!("wait for funding tx: {e}"))?;

    let funding_utxo = fetch_utxo_by_script(provider, &funding_txid, &guard_spk)?;
    let funding_vout = funding_utxo.outpoint.vout;
    let funding_amount = funding_utxo.explicit_amount();
    if funding_amount <= args.fee_reserve {
        return Err(format!(
            "funding amount {} is not greater than fee reserve {}",
            funding_amount, args.fee_reserve
        ));
    }

    let oracle_signature = decode_signature(&feed.signature)?;

    let mut spend_txid = None;
    let mut spent_tick = None;
    let mut attempt = 0usize;

    while spend_txid.is_none() {
        attempt += 1;

        let tick = select_unspent_tick(&client, &base_url, &rpc, &tried_tick_outpoints).await?;
        let tick_txid =
            elements::Txid::from_str(&tick.txid).map_err(|e| format!("invalid tick txid: {e}"))?;
        let tick_utxo = fetch_utxo_by_outpoint(provider, &tick_txid, tick.vout as u32)?;
        let tick_asset = tick_utxo.explicit_asset();
        let tick_asset_id = tick_asset.into_inner().to_byte_array();
        if tick_asset_id != guard_tick_asset_id {
            return Err(format!(
                "tick asset mismatch: funded guard for {} but selected tick {}:{} uses {}",
                hex::encode(guard_tick_asset_id),
                tick.txid,
                tick.vout,
                hex::encode(tick_asset_id)
            ));
        }

        let guard_args = OraclePriceGuardArguments {
            oracle_pubkey: oracle_xonly.serialize(),
            feed_id: args.feed_id,
            max_price: args.threshold,
            claim_script_hash,
            tick_asset_id: guard_tick_asset_id,
        };
        let guard_program = OraclePriceGuardProgram::new(guard_args.clone());

        let mut ft = FinalTransaction::new();
        let guard_witness = OraclePriceGuardWitness {
            price: feed.price,
            timestamp: feed.timestamp,
            valid_until: feed.valid_until,
            oracle_signature,
        };
        let guard_input = ProgramInput::new(
            Box::new(guard_program.get_program().clone()),
            Box::new(guard_witness),
        );
        ft.add_program_input(
            PartialInput::new(funding_utxo.clone()),
            guard_input,
            RequiredSignature::None,
        );

        let tick_program = TimestampCovenantProgram::new(TimestampCovenantArguments {
            issuer_script_hash,
            tick_asset_id,
        });
        let tick_input = ProgramInput::new(
            Box::new(tick_program.get_program().clone()),
            Box::new(TimestampCovenantWitness {}),
        );
        ft.add_program_input(
            PartialInput::new(tick_utxo),
            tick_input,
            RequiredSignature::None,
        );

        ft.add_output(PartialOutput::new(
            issuer_spk.clone(),
            tick.amount as u64,
            tick_asset,
        ));
        ft.add_output(PartialOutput::new(
            claim_spk.clone(),
            funding_amount.saturating_sub(args.fee_reserve),
            funding_utxo.explicit_asset(),
        ));

        if args.interactive {
            print_simplicity_parameters(
                &guard_args,
                &feed,
                &oracle_signature,
                &issuer_script_hash,
                funding_amount,
                args.fee_reserve,
                tick.amount as u64,
                tick.vout as u32,
            );

            print_status_block(
                "Guarded Spend Prepared",
                ANSI_MAGENTA,
                &[
                    ("Attempt", attempt.to_string()),
                    (
                        "Funding Input",
                        format!("{}:{}", funding_txid, funding_vout),
                    ),
                    ("Tick Input", format!("{}:{}", tick.txid, tick.vout)),
                ],
            );
            interactive_outpoint_checkpoint(
                &rpc,
                &funding_txid,
                funding_vout,
                "guarded spend inputs",
            )?;
            interactive_outpoint_checkpoint(
                &rpc,
                &tick_txid,
                tick.vout as u32,
                "guarded spend inputs",
            )?;
        }

        let spend_result = catch_unwind(AssertUnwindSafe(|| signer.broadcast(&ft)));
        match spend_result {
            Ok(Ok(txid)) => {
                spend_txid = Some(txid);
                spent_tick = Some(tick);
            }
            Ok(Err(e)) => {
                let message = e.to_string();
                if is_missing_or_spent_error(&message) {
                    tried_tick_outpoints.insert(format!("{}:{}", tick.txid, tick.vout));
                    print_warning(&format!(
                        "Tick {}:{} was spent before broadcast; retrying with a new tick.",
                        tick.txid, tick.vout
                    ));
                    continue;
                }
                return Err(format!("broadcast guarded spend: {message}"));
            }
            Err(_) => return Err("guarded spend panicked during program execution".to_string()),
        };
    }

    let spend_txid = spend_txid.expect("set on successful broadcast");
    let tick = spent_tick.expect("set on successful broadcast");

    print_status_block(
        "Oracle Guard Spent",
        ANSI_GREEN,
        &[
            ("Spend Tx", spend_txid.to_string()),
            ("Feed Id", feed.id.to_string()),
            ("Price", feed.price.to_string()),
            ("Threshold", args.threshold.to_string()),
            ("Tick Input", format!("{}:{}", tick.txid, tick.vout)),
        ],
    );

    if args.interactive {
        interactive_tx_checkpoint(&rpc, &spend_txid, "guarded spend transaction")?;
    }

    Ok(())
}

fn interactive_tx_checkpoint(rpc: &RpcClient, txid: &Txid, step: &str) -> Result<(), String> {
    loop {
        print_action_prompt(step, &format!("tx {}", txid), true);
        io::stdout()
            .flush()
            .map_err(|e| format!("flush stdout: {e}"))?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("read input: {e}"))?;

        match input.trim().to_ascii_lowercase().as_str() {
            "i" | "inspect" => inspect_transaction(rpc, txid)?,
            "c" | "continue" | "" => return Ok(()),
            "q" | "quit" | "n" | "no" => {
                return Err(format!("aborted by user at {step}"));
            }
            _ => print_warning("Please enter i, c, or q."),
        }
    }
}

fn interactive_outpoint_checkpoint(
    rpc: &RpcClient,
    txid: &Txid,
    vout: u32,
    step: &str,
) -> Result<(), String> {
    loop {
        print_action_prompt(step, &format!("outpoint {}:{}", txid, vout), false);
        io::stdout()
            .flush()
            .map_err(|e| format!("flush stdout: {e}"))?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("read input: {e}"))?;

        match input.trim().to_ascii_lowercase().as_str() {
            "i" | "inspect" => inspect_transaction(rpc, txid)?,
            "u" | "unspent" => inspect_outpoint_unspent(rpc, txid, vout)?,
            "c" | "continue" | "" => return Ok(()),
            "q" | "quit" | "n" | "no" => {
                return Err(format!("aborted by user at {step}"));
            }
            _ => print_warning("Please enter i, u, c, or q."),
        }
    }
}

fn inspect_transaction(rpc: &RpcClient, txid: &Txid) -> Result<(), String> {
    let tx: Value = rpc
        .call(
            "getrawtransaction",
            &[Value::String(txid.to_string()), Value::Bool(true)],
        )
        .map_err(|e| format!("getrawtransaction {txid}: {e}"))?;

    print_scanner_view(&tx);

    Ok(())
}

fn print_simplicity_parameters(
    guard_args: &OraclePriceGuardArguments,
    feed: &FeedResponse,
    oracle_signature: &[u8; 64],
    issuer_script_hash: &[u8; 32],
    funding_amount: u64,
    fee_reserve: u64,
    tick_amount: u64,
    tick_vout: u32,
) {
    print_section_header("Simplicity Parameters", ANSI_MAGENTA);
    print_subsection(
        "OraclePriceGuardArguments",
        &[
            ("oracle_pubkey", hex::encode(guard_args.oracle_pubkey)),
            ("feed_id", guard_args.feed_id.to_string()),
            ("max_price", guard_args.max_price.to_string()),
            (
                "claim_script_hash",
                hex::encode(guard_args.claim_script_hash),
            ),
        ],
    );
    print_subsection(
        "OraclePriceGuardWitness",
        &[
            ("price", feed.price.to_string()),
            ("timestamp", feed.timestamp.to_string()),
            ("valid_until", feed.valid_until.to_string()),
            (
                "oracle_signature",
                format!(
                    "{}  ({} bytes)",
                    abbreviate_middle(&hex::encode(oracle_signature), 18, 16),
                    oracle_signature.len()
                ),
            ),
        ],
    );
    print_subsection(
        "TimestampCovenant",
        &[
            ("issuer_script_hash", hex::encode(issuer_script_hash)),
            ("witness", "empty".to_string()),
        ],
    );
    print_subsection(
        "Spend Summary",
        &[
            ("funding_amount", funding_amount.to_string()),
            ("fee_reserve", fee_reserve.to_string()),
            (
                "claim_amount",
                funding_amount.saturating_sub(fee_reserve).to_string(),
            ),
            ("tick_amount", tick_amount.to_string()),
            ("tick_vout", tick_vout.to_string()),
        ],
    );
    print_section_footer();
}

fn print_scanner_view(tx: &Value) {
    print_section_header("Transaction Scanner View", ANSI_CYAN);
    print_subsection(
        "Summary",
        &[
            ("txid", field_str(tx, "txid")),
            ("confirmations", field_num(tx, "confirmations")),
            ("size", field_num(tx, "size")),
            ("vsize", field_num(tx, "vsize")),
            ("weight", field_num(tx, "weight")),
        ],
    );

    if let Some(vin) = tx.get("vin").and_then(Value::as_array) {
        println!(
            "{}",
            section_label(&format!("Inputs ({})", vin.len()), ANSI_YELLOW)
        );
        for (i, input) in vin.iter().enumerate() {
            let prev_txid = input
                .get("txid")
                .and_then(Value::as_str)
                .unwrap_or("<coinbase>");
            let prev_vout = input
                .get("vout")
                .map(value_to_short_string)
                .unwrap_or_else(|| "-".to_string());
            let sequence = input
                .get("sequence")
                .map(value_to_short_string)
                .unwrap_or_else(|| "-".to_string());

            print_row(
                &format!("vin[{i}] prevout"),
                &format!("{}:{}", prev_txid, prev_vout),
            );
            print_row(&format!("vin[{i}] sequence"), &sequence);

            if let Some(wit) = input.get("txinwitness").and_then(Value::as_array) {
                print_row(&format!("vin[{i}] witness items"), &wit.len().to_string());
                for (widx, item) in wit.iter().enumerate() {
                    let hex_item = item.as_str().unwrap_or("");
                    print_row(
                        &format!("  w[{widx}]"),
                        &format!(
                            "{:>4} bytes  {}",
                            hex_bytes_len(hex_item),
                            abbreviate_middle(hex_item, 16, 14)
                        ),
                    );
                }
                println!();
            }
        }
    }

    if let Some(vout) = tx.get("vout").and_then(Value::as_array) {
        println!(
            "{}",
            section_label(&format!("Outputs ({})", vout.len()), ANSI_GREEN)
        );
        for out in vout {
            let n = out
                .get("n")
                .map(value_to_short_string)
                .unwrap_or_else(|| "?".to_string());
            let value = out
                .get("value")
                .map(value_to_short_string)
                .unwrap_or_else(|| "?".to_string());
            let asset = out
                .get("asset")
                .map(value_to_short_string)
                .unwrap_or_else(|| "?".to_string());
            let spk_type = out
                .get("scriptPubKey")
                .and_then(|spk| spk.get("type"))
                .map(value_to_short_string)
                .unwrap_or_else(|| "?".to_string());

            print_row(&format!("vout[{n}] value"), &value);
            print_row(&format!("vout[{n}] asset"), &asset);
            print_row(&format!("vout[{n}] type"), &spk_type);
            println!();
        }
    }

    print_section_footer();
}

fn field_str(obj: &Value, key: &str) -> String {
    obj.get(key)
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string()
}

fn field_num(obj: &Value, key: &str) -> String {
    obj.get(key)
        .map(value_to_short_string)
        .unwrap_or_else(|| "-".to_string())
}

fn value_to_short_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => {
            if let Some(f) = v.as_f64() {
                if f.fract() != 0.0 {
                    // Fixed-point with up to 8 decimal places, strip trailing zeros
                    let s = format!("{:.8}", f);
                    let s = s.trim_end_matches('0');
                    let s = s.trim_end_matches('.');
                    return s.to_string();
                }
            }
            v.to_string()
        }
        Value::String(v) => v.clone(),
        _ => value.to_string(),
    }
}

fn abbreviate_middle(value: &str, prefix: usize, suffix: usize) -> String {
    if value.len() <= prefix + suffix + 3 {
        return value.to_string();
    }

    format!(
        "{}...{}",
        &value[..prefix.min(value.len())],
        &value[value.len() - suffix.min(value.len())..]
    )
}

fn hex_bytes_len(hex_value: &str) -> usize {
    if hex_value.is_empty() {
        return 0;
    }
    hex::decode(hex_value).map(|b| b.len()).unwrap_or(0)
}

fn inspect_outpoint_unspent(rpc: &RpcClient, txid: &Txid, vout: u32) -> Result<(), String> {
    let response: Value = rpc
        .call(
            "gettxout",
            &[
                Value::String(txid.to_string()),
                Value::from(vout),
                Value::Bool(true),
            ],
        )
        .map_err(|e| format!("gettxout {txid}:{vout}: {e}"))?;

    if response.is_null() {
        print_status_block(
            "Outpoint Status",
            ANSI_RED,
            &[
                ("Outpoint", format!("{}:{}", txid, vout)),
                ("Status", "spent or unavailable".to_string()),
            ],
        );
    } else {
        print_status_block(
            "Outpoint Status",
            ANSI_GREEN,
            &[
                (
                    "Outpoint",
                    format!("{}:{}", abbreviate_middle(&txid.to_string(), 16, 12), vout),
                ),
                (
                    "Value",
                    response
                        .get("value")
                        .map(value_to_short_string)
                        .unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "Asset",
                    response
                        .get("asset")
                        .map(value_to_short_string)
                        .map(|value| abbreviate_middle(&value, 18, 14))
                        .unwrap_or_else(|| "-".to_string()),
                ),
                (
                    "Script Type",
                    response
                        .get("scriptPubKey")
                        .and_then(|spk| spk.get("type"))
                        .map(value_to_short_string)
                        .unwrap_or_else(|| "-".to_string()),
                ),
            ],
        );
    }

    Ok(())
}

fn print_status_block(title: &str, color: &str, rows: &[(&str, String)]) {
    print_section_header(title, color);
    for (label, value) in rows {
        print_row(label, value);
    }
    print_section_footer();
}

fn print_section_header(title: &str, color: &str) {
    let border = format!("+{:-<74}+", "");
    println!();
    println!("{}{}{}", color, border, ANSI_RESET);
    println!(
        "{}| {:<72} |{}",
        color,
        format!("{}{}{}", ANSI_BOLD, title, ANSI_RESET),
        ANSI_RESET
    );
    println!("{}{}{}", color, border, ANSI_RESET);
}

fn print_section_footer() {
    println!("{}+{:-<74}+{}", ANSI_DIM, "", ANSI_RESET);
}

fn print_subsection(title: &str, rows: &[(&str, String)]) {
    println!("{}{}{}", ANSI_BOLD, title, ANSI_RESET);
    for (label, value) in rows {
        print_row(label, value);
    }
    println!();
}

fn print_row(label: &str, value: &str) {
    println!("  {}{:<20}{} {}", ANSI_CYAN, label, ANSI_RESET, value);
}

fn print_action_prompt(step: &str, subject: &str, tx_prompt: bool) {
    let actions = if tx_prompt {
        format!(
            "{}i{} inspect   {}c{} continue   {}q{} quit",
            ANSI_CYAN, ANSI_RESET, ANSI_GREEN, ANSI_RESET, ANSI_RED, ANSI_RESET
        )
    } else {
        format!(
            "{}i{} inspect   {}u{} unspent   {}c{} continue   {}q{} quit",
            ANSI_CYAN,
            ANSI_RESET,
            ANSI_YELLOW,
            ANSI_RESET,
            ANSI_GREEN,
            ANSI_RESET,
            ANSI_RED,
            ANSI_RESET
        )
    };

    print!(
        "\n{}[{}]{} {}{}{}\n  {}\n  {}> {}",
        ANSI_BOLD, step, ANSI_RESET, ANSI_DIM, subject, ANSI_RESET, actions, ANSI_BOLD, ANSI_RESET,
    );
}

fn print_warning(message: &str) {
    println!("{}warning:{} {}", ANSI_YELLOW, ANSI_RESET, message);
}

fn section_label(title: &str, color: &str) -> String {
    format!("\n{}{}{}", color, title, ANSI_RESET)
}

fn new_timekeeper_rpc(config: &Config) -> Result<RpcClient, String> {
    let auth = Auth::UserPass(
        config.service.timekeeper.rpc_user.clone(),
        config.service.timekeeper.rpc_password.clone(),
    );

    RpcClient::new(&config.service.timekeeper.rpc_url, auth)
        .map_err(|e| format!("connect timekeeper rpc: {e}"))
}

async fn select_unspent_tick(
    client: &Client,
    base_url: &str,
    rpc: &RpcClient,
    excluded_outpoints: &HashSet<String>,
) -> Result<TickItem, String> {
    const PAGE_SIZE: usize = 100;

    let mut offset = 0usize;

    loop {
        let ticks: TickListResponse = get_json(
            client,
            &format!("{base_url}/price-oracle/timekeeper/ticks?limit={PAGE_SIZE}&offset={offset}"),
        )
        .await?;

        if ticks.items.is_empty() {
            break;
        }

        for tick in ticks.items {
            let outpoint = format!("{}:{}", tick.txid, tick.vout);
            if excluded_outpoints.contains(&outpoint) {
                continue;
            }
            if tick_is_unspent(rpc, &tick)? {
                return Ok(tick);
            }
        }

        offset += PAGE_SIZE;
        if offset as i64 >= ticks.total {
            break;
        }
    }

    Err("no unspent timekeeper tick UTXOs available from service".to_string())
}

fn tick_is_unspent(rpc: &RpcClient, tick: &TickItem) -> Result<bool, String> {
    let response: Value = rpc
        .call(
            "gettxout",
            &[
                Value::String(tick.txid.clone()),
                Value::from(tick.vout),
                Value::Bool(true),
            ],
        )
        .map_err(|e| format!("gettxout {}:{}: {e}", tick.txid, tick.vout))?;

    Ok(!response.is_null())
}

fn is_missing_or_spent_error(message: &str) -> bool {
    message.contains("bad-txns-inputs-missingorspent")
}

async fn get_json<T: for<'de> Deserialize<'de>>(client: &Client, url: &str) -> Result<T, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("read response body {url}: {e}"))?;
    if !status.is_success() {
        return Err(format!("GET {url} failed with {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("parse JSON from {url}: {e}"))
}

fn decode_signature(signature_hex: &str) -> Result<[u8; 64], String> {
    let bytes = hex::decode(signature_hex).map_err(|e| format!("invalid signature hex: {e}"))?;
    let array: [u8; 64] = bytes
        .try_into()
        .map_err(|_| "oracle signature must be 64 bytes".to_string())?;
    Ok(array)
}

fn fund_guard(
    signer: &Signer,
    guard_spk: &elements::Script,
    fund_amount: u64,
) -> Result<Txid, String> {
    let network = *signer.get_provider().get_network();
    let mut ft = FinalTransaction::new();
    ft.add_output(PartialOutput::new(
        guard_spk.clone(),
        fund_amount,
        network.policy_asset(),
    ));
    signer
        .broadcast(&ft)
        .map_err(|e| format!("fund oracle guard: {e}"))
}

fn fetch_utxo_by_script(
    provider: &dyn ProviderTrait,
    txid: &Txid,
    script: &elements::Script,
) -> Result<UTXO, String> {
    let tx = provider
        .fetch_transaction(txid)
        .map_err(|e| format!("fetch transaction {txid}: {e}"))?;

    tx.output
        .iter()
        .enumerate()
        .find(|(_, output)| output.script_pubkey == *script)
        .map(|(vout, output)| UTXO {
            outpoint: OutPoint {
                txid: *txid,
                vout: vout as u32,
            },
            txout: output.clone(),
            secrets: None,
        })
        .ok_or_else(|| format!("no output paying to target script in tx {txid}"))
}

fn fetch_utxo_by_outpoint(
    provider: &dyn ProviderTrait,
    txid: &Txid,
    vout: u32,
) -> Result<UTXO, String> {
    let tx = provider
        .fetch_transaction(txid)
        .map_err(|e| format!("fetch transaction {txid}: {e}"))?;
    let txout = tx
        .output
        .get(vout as usize)
        .ok_or_else(|| format!("missing vout {vout} in tx {txid}"))?
        .clone();

    Ok(UTXO {
        outpoint: OutPoint { txid: *txid, vout },
        txout,
        secrets: None,
    })
}
