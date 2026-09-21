//! Glicko-2 rating API for kill-based game events.
//!
//! Build: `cargo build --release`
//! Run:   `glicko-api [--bind 0.0.0.0:8080] [--events path.json]`

mod api;
mod engine;
mod glicko;
mod model;
mod store;

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use serde::Deserialize;

use api::AppState;
use model::EventInput;
use store::Store;

const USAGE: &str = "\
glicko-api - Glicko-2 rating API

USAGE:
    glicko-api [OPTIONS]

OPTIONS:
    --bind <ADDR>       Listen address [default: 127.0.0.1:8080]
                        (env: GLICKO_BIND)
    --events <FILE>     Import this JSON event file before serving
                        (env: GLICKO_EVENTS)
    -h, --help          Print this help
";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bind = std::env::var("GLICKO_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let mut events_file = std::env::var("GLICKO_EVENTS").ok();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => bind = args.next().ok_or("--bind needs an address")?,
            "--events" => events_file = Some(args.next().ok_or("--events needs a file path")?),
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}").into()),
        }
    }
    let addr: SocketAddr = bind.parse()?;

    let mut store = Store::new();
    if let Some(path) = events_file {
        let raw = std::fs::read_to_string(&path)?;
        let events: Vec<EventInput> = parse_event_file(&raw)?;
        let report = store.import(events).map_err(|e| format!("{path}: {e}"))?;
        println!(
            "loaded {path}: {} imported, {} updated, {} skipped, {} events total, {} players",
            report.imported, report.updated, report.skipped, report.events_total, report.players
        );
    }

    let state: AppState = Arc::new(RwLock::new(store));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}\n(on Windows a port already held by another process surfaces as `access denied`; pass --bind to pick another)"))?;
    println!("listening on http://{addr}");
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}

/// A dump is either a bare array or an object with an `events` array.
fn parse_event_file(raw: &str) -> Result<Vec<EventInput>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Wrapped {
        events: Vec<EventInput>,
    }

    let value: serde_json::Value = serde_json::from_str(raw)?;
    if value.is_array() {
        serde_json::from_value(value)
    } else {
        Wrapped::deserialize(value).map(|w| w.events)
    }
}
