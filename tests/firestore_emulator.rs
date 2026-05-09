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

    // get_schema_snapshot — posts.author should reference users.
    // Tabularis expects Vec<TableSchema> (verified in src-tauri plugin bridge).
    let snap = p.call("get_schema_snapshot", json!({ "params": {} }));
    let tables = snap["result"].as_array().expect("snapshot is an array");
    let posts = tables
        .iter()
        .find(|t| t["name"] == "posts")
        .expect("posts in snapshot");
    let fks = posts["foreign_keys"]
        .as_array()
        .expect("posts foreign_keys");
    let fk_to_users = fks
        .iter()
        .find(|fk| fk["column_name"] == "author" && fk["ref_table"] == "users");
    assert!(
        fk_to_users.is_some(),
        "expected posts.author -> users FK: {snap}"
    );
}

#[test]
#[ignore]
fn phase3_crud_against_emulator() {
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set");
    let project =
        std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();

    let init = p.call(
        "initialize",
        json!({
            "settings": { "project_id": project, "emulator_host": host, "sample_size": 50 }
        }),
    );
    assert!(init.get("error").is_none(), "initialize: {init}");

    // Prime the column cache so insert validation has schema to consult.
    let _ = p.call("get_columns", json!({ "table": "users" }));

    // Insert with explicit id.
    let ins = p.call(
        "insert_record",
        json!({
            "table": "users",
            "data": { "id": "crud-test-doc", "email": "crud@x.de", "active": true }
        }),
    );
    assert_eq!(ins["result"], json!(1u64), "insert: {ins}");

    // Read-back roundtrip — bool/string preserved.
    let q = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT id, email, active FROM \"users\" WHERE id = 'crud-test-doc'"
        }),
    );
    let row = q["result"]["rows"][0].as_array().unwrap();
    assert_eq!(row[0], json!("crud-test-doc"));
    assert_eq!(row[1], json!("crud@x.de"));
    assert_eq!(row[2], json!(true));

    // Update single field.
    let upd = p.call(
        "update_record",
        json!({
            "table": "users",
            "pk_col": "id",
            "pk_val": "crud-test-doc",
            "col_name": "email",
            "new_val": "updated@x.de",
        }),
    );
    assert_eq!(upd["result"], json!(1u64), "update: {upd}");

    let q2 = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT email FROM \"users\" WHERE id = 'crud-test-doc'"
        }),
    );
    assert_eq!(q2["result"]["rows"][0][0], json!("updated@x.de"));

    // Update on the synthetic id triggers the rename pattern.
    let rename = p.call(
        "update_record",
        json!({
            "table": "users",
            "pk_col": "id",
            "pk_val": "crud-test-doc",
            "col_name": "id",
            "new_val": "crud-test-renamed",
        }),
    );
    assert_eq!(rename["result"], json!(1u64), "rename: {rename}");

    let q3 = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT id, email FROM \"users\" WHERE id IN ('crud-test-doc', 'crud-test-renamed')"
        }),
    );
    let rows3 = q3["result"]["rows"].as_array().unwrap();
    assert_eq!(rows3.len(), 1, "exactly one doc after rename: {q3}");
    assert_eq!(rows3[0][0], json!("crud-test-renamed"));
    assert_eq!(rows3[0][1], json!("updated@x.de"));

    // Cleanup.
    let del = p.call(
        "delete_record",
        json!({
            "table": "users",
            "pk_col": "id",
            "pk_val": "crud-test-renamed",
        }),
    );
    assert_eq!(del["result"], json!(1u64), "delete: {del}");
}

#[test]
#[ignore]
fn rename_collision_returns_structured_error() {
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set");
    let project =
        std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();
    let _ = p.call(
        "initialize",
        json!({"settings": { "project_id": project, "emulator_host": host, "sample_size": 50 }}),
    );

    // Try to rename alice → bob (both seeded). Must fail with -32602.
    let r = p.call(
        "update_record",
        json!({
            "table": "users",
            "pk_col": "id",
            "pk_val": "alice",
            "col_name": "id",
            "new_val": "bob",
        }),
    );
    assert_eq!(r["error"]["code"], json!(-32602), "{r}");
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "{r}"
    );
}

#[test]
#[ignore]
fn auto_generated_doc_id_when_id_omitted() {
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set");
    let project =
        std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();
    let _ = p.call(
        "initialize",
        json!({"settings": { "project_id": project, "emulator_host": host, "sample_size": 50 }}),
    );

    // Insert with no `id` field — Firestore should auto-generate.
    let ins = p.call(
        "insert_record",
        json!({
            "table": "users",
            "data": { "email": "autogen@x.de", "active": true }
        }),
    );
    assert_eq!(ins["result"], json!(1u64), "{ins}");

    // Find the generated doc to clean up.
    let q = p.call(
        "execute_query",
        json!({
            "params": {},
            "query": "SELECT id FROM \"users\" WHERE email = 'autogen@x.de'"
        }),
    );
    let id = q["result"]["rows"][0][0]
        .as_str()
        .expect("autogen id should be a string")
        .to_string();
    assert!(!id.is_empty());
    assert_ne!(id, "alice");
    assert_ne!(id, "bob");

    let _ = p.call(
        "delete_record",
        json!({"table": "users", "pk_col": "id", "pk_val": id}),
    );
}

#[test]
#[ignore]
fn explain_plan_shape_matches_tabularis_contract() {
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set");
    let project =
        std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();
    let _ = p.call(
        "initialize",
        json!({"settings": { "project_id": project, "emulator_host": host, "sample_size": 50 }}),
    );

    let r = p.call(
        "explain_query",
        json!({
            "params": {},
            "query": "SELECT * FROM \"users\" LIMIT 5",
            "analyze": true,
        }),
    );
    let plan = &r["result"];
    let root = &plan["root"];
    // Shape contract — these are what Tabularis' deserializer checks for.
    // The Firestore emulator doesn't always return execution_stats even with
    // analyze=true (emulator-vs-production gap), so don't assert on
    // has_analyze_data / actual_rows here — verify the wire shape only.
    assert!(root.is_object(), "root is required: {r}");
    assert_eq!(root["relation"], json!("users"));
    assert!(root["extra"].is_object());
    assert!(root["children"].is_array());
    assert_eq!(plan["driver"], json!("firestore"));
    assert!(plan["has_analyze_data"].is_boolean());
    assert!(plan["original_query"].as_str().unwrap().contains("users"));
    let extra = root["extra"].as_object().unwrap();
    assert!(extra.contains_key("documents_scanned"));
    assert!(extra.contains_key("limit"));
}

#[test]
#[ignore]
fn schema_overrides_required_field_blocks_insert() {
    use std::io::Write;
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set");
    let project =
        std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let dir = std::env::temp_dir().join("firestore-plugin-it-schemas");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut f = std::fs::File::create(dir.join(format!("{project}.json"))).unwrap();
    writeln!(
        f,
        r#"{{"collections":{{"users":{{"fields":{{"region":{{"required":true}}}}}}}}}}"#
    )
    .unwrap();

    let mut p = Plugin::spawn();
    let _ = p.call(
        "initialize",
        json!({"settings": {
            "project_id": project,
            "emulator_host": host,
            "sample_size": 50,
            "schema_overrides_dir": dir.to_str().unwrap(),
        }}),
    );
    let _ = p.call("get_columns", json!({ "table": "users" }));

    // Insert without `region` — should fail per the override.
    let ins = p.call(
        "insert_record",
        json!({
            "table": "users",
            "data": { "id": "override-test", "email": "ot@x.de" }
        }),
    );
    assert_eq!(ins["error"]["code"], json!(-32602), "{ins}");
    assert!(
        ins["error"]["message"]
            .as_str()
            .unwrap()
            .contains("region"),
        "{ins}"
    );

    // With region — should succeed; clean up.
    let ok = p.call(
        "insert_record",
        json!({
            "table": "users",
            "data": { "id": "override-test", "email": "ot@x.de", "region": "eu" }
        }),
    );
    assert_eq!(ok["result"], json!(1u64));
    let _ = p.call(
        "delete_record",
        json!({"table": "users", "pk_col": "id", "pk_val": "override-test"}),
    );

    let _ = std::fs::remove_dir_all(&dir);
}
