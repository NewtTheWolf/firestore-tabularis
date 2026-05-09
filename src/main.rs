//! Entry point: read JSON-RPC lines from stdin, dispatch, write responses.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

mod cache;
mod client;
mod coercion;
mod error;
mod firestore_error;
mod firestore_filter;
mod handlers;
mod models;
mod query_parser;
mod rpc;
mod schema_infer;
mod schema_overrides;
mod state;
mod utils;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some(response) = rpc::handle_line(trimmed).await else {
            // JSON-RPC notification — no reply expected.
            continue;
        };
        let mut body = match serde_json::to_string(&response) {
            Ok(s) => s,
            Err(err) => format!(
                "{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-32603,\"message\":\"serialization failed: {err}\"}},\"id\":null}}",
            ),
        };
        body.push('\n');
        if stdout.write_all(body.as_bytes()).await.is_err() {
            break;
        }
        let _ = stdout.flush().await;
    }
}
