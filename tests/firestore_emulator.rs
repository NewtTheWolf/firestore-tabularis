//! End-to-end: spawn the plugin binary as a child process and drive it over stdio.
//!
//! Skipped unless `FIRESTORE_EMULATOR_HOST` is set. CI starts the emulator
//! and seeds a small fixture before running `cargo test --ignored`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Plugin {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
}

impl Plugin {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_firestore-plugin"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn plugin");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Plugin {
            child,
            stdin,
            out: BufReader::new(stdout),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let line = format!("{}\n", req);
        self.stdin.write_all(line.as_bytes()).expect("write");
        self.stdin.flush().expect("flush");

        let mut buf = String::new();
        self.out.read_line(&mut buf).expect("read");
        serde_json::from_str(&buf).expect("parse response")
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn emulator_host() -> Option<String> {
    std::env::var("FIRESTORE_EMULATOR_HOST").ok()
}

#[test]
#[ignore]
fn end_to_end_against_emulator() {
    let host = emulator_host().expect(
        "FIRESTORE_EMULATOR_HOST not set — set it (e.g. localhost:8080) and seed the fixture",
    );
    let project =
        std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();

    let init = p.call(
        "initialize",
        json!({
            "settings": { "project_id": project, "emulator_host": host, "sample_size": 50 }
        }),
    );
    assert!(init.get("error").is_none(), "initialize failed: {init}");

    let test = p.call("test_connection", json!({ "params": {} }));
    assert_eq!(
        test["result"]["success"],
        Value::Bool(true),
        "test_connection: {test}"
    );

    let dbs = p.call("get_databases", json!({ "params": {} }));
    assert!(dbs["result"].is_array(), "get_databases: {dbs}");

    let tables = p.call("get_tables", json!({ "params": {} }));
    assert!(tables["result"].is_array(), "get_tables: {tables}");
}
