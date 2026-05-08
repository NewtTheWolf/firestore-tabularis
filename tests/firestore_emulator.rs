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

#[test]
#[ignore]
fn phase2_query_layer_against_emulator() {
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set");
    let project =
        std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();

    // Initialize and connect.
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

    // WHERE eq
    let q1 = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT * FROM \"users\" WHERE email = 'alice@x.de'"
        }),
    );
    let rows1 = q1["result"]["rows"].as_array().unwrap();
    assert_eq!(rows1.len(), 1, "alice should match: {q1}");

    // WHERE int comparison + IN
    let q2 = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT * FROM \"posts\" WHERE views > 100 AND status IN ('published', 'draft')"
        }),
    );
    let rows2 = q2["result"]["rows"].as_array().unwrap();
    assert!(!rows2.is_empty(), "expected post1: {q2}");

    // OR with parens + ARRAY_CONTAINS
    let q3 = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT * FROM \"posts\" WHERE (priority = 'high' OR priority = 'urgent') AND ARRAY_CONTAINS(tags, 'launch')"
        }),
    );
    assert!(
        q3.get("error").is_none(),
        "OR + ARRAY_CONTAINS failed: {q3}"
    );

    // total_count
    let q4 = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT * FROM \"users\" LIMIT 1"
        }),
    );
    let total = q4["result"]["total_count"].as_u64().unwrap();
    assert_eq!(total, 2, "expected 2 seeded users: {q4}");

    // Pagination — first 1 row, then next 1 row
    let q5a = p.call(
        "execute_query",
        json!({
            "params": {}, "query": "SELECT * FROM \"users\" ORDER BY email ASC LIMIT 1 OFFSET 0"
        }),
    );
    let q5b = p.call(
        "execute_query",
        json!({
            "params": {}, "query": "SELECT * FROM \"users\" ORDER BY email ASC LIMIT 1 OFFSET 1"
        }),
    );
    let id_a = q5a["result"]["rows"][0][0].as_str().unwrap().to_string();
    let id_b = q5b["result"]["rows"][0][0].as_str().unwrap().to_string();
    assert_ne!(
        id_a, id_b,
        "pagination should produce disjoint pages: {q5a} {q5b}"
    );

    // get_schema_snapshot — posts.author should reference users
    let snap = p.call("get_schema_snapshot", json!({ "params": {} }));
    let fks = &snap["result"]["foreign_keys"]["posts"];
    assert!(fks.is_array(), "expected posts foreign_keys array: {snap}");
    let fk_to_users = fks
        .as_array()
        .unwrap()
        .iter()
        .find(|fk| fk["from_column"] == "author" && fk["to_table"] == "users");
    assert!(
        fk_to_users.is_some(),
        "expected posts.author -> users FK: {snap}"
    );
}
