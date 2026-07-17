//! The MCP server driven as a real subprocess over stdin/stdout, the way a
//! client speaks to it. The in-crate unit tests exercise `dispatch` directly;
//! only this proves the transport itself — framing, the version handshake, the
//! error codes, and that a stray line does not take the session down.
//!
//! Hermetic: each run points `HOME` at a fresh temp dir and strips any
//! credential env, so the server finds an empty archive and no cookies. That
//! makes the credential gate deterministic regardless of the developer's box.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

fn scratch_home() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kicau-mcp-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Spawn `kicau mcp <args>`, feed `requests` (one JSON-RPC message per line),
/// and return every stdout line parsed as JSON. Panics if any stdout line is
/// not valid JSON — that is the contamination a real client cannot survive.
fn drive(args: &[&str], requests: &[&str]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kicau"))
        .arg("mcp")
        .args(args)
        .env("HOME", scratch_home())
        .env_remove("KICAU_AUTH_TOKEN")
        .env_remove("KICAU_CT0")
        .env_remove("AUTH_TOKEN")
        .env_remove("CT0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn kicau mcp");

    {
        let mut stdin = child.stdin.take().unwrap();
        for line in requests {
            writeln!(stdin, "{line}").unwrap();
        }
        // Dropping stdin closes it, so the server reaches EOF and exits.
    }

    let out = child.wait_with_output().expect("wait for kicau mcp");
    assert!(out.status.success(), "server exited with {:?}", out.status);
    String::from_utf8(out.stdout)
        .expect("stdout is utf-8")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every stdout line must be valid JSON"))
        .collect()
}

const ACCEPTED_VERSIONS: &[&str] = &[
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];

#[test]
fn initialize_negotiates_a_version_clients_accept() {
    let responses = drive(
        &[],
        &[r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#],
    );
    assert_eq!(responses.len(), 1);
    let version = responses[0]["result"]["protocolVersion"].as_str().unwrap();
    assert!(
        ACCEPTED_VERSIONS.contains(&version),
        "clients hang up on an unrecognised version: {version}"
    );
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "kicau");
}

#[test]
fn tools_list_advertises_all_five_tools() {
    let responses = drive(&[], &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#]);
    let tools = responses[0]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);
    assert!(tools.iter().any(|t| t["name"] == "read_tweet"));
}

#[test]
fn read_tweet_without_credentials_refuses_as_a_result_not_a_crash() {
    // The scratch HOME has no config.toml, so no cookies resolve. read_tweet ran
    // and could not answer: isError, and the server stays up.
    let responses = drive(
        &[],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_tweet","arguments":{"tweet":"1"}}}"#,
        ],
    );
    assert_eq!(responses[0]["result"]["isError"], true);
    assert!(
        responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("kicau init")
    );
}

#[test]
fn error_codes_follow_the_spec() {
    let responses = drive(
        &[],
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"bogus/method"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nope"}}"#,
        ],
    );
    assert_eq!(responses[0]["error"]["code"], -32601, "unknown method");
    assert_eq!(responses[1]["error"]["code"], -32602, "unknown tool");
}

#[test]
fn a_notification_gets_no_reply_and_garbage_does_not_crash_the_server() {
    let responses = drive(
        &[],
        &[
            // No id: a notification. The server acts, answers nothing.
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            // Not JSON: logged to stderr, skipped, server keeps serving.
            "this is not json at all",
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"archive_stats"}}"#,
        ],
    );
    assert_eq!(
        responses.len(),
        1,
        "only the one request with an id answers"
    );
    assert_eq!(responses[0]["id"], 9);
    assert!(
        responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("tweets:")
    );
}
