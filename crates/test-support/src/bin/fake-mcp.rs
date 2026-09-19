use serde_json::{Value, json};
use std::env;
use std::io::{self, BufRead, Write};
use std::thread;
use std::time::Duration;

/// A deterministic, protocol-shaped target used only by offline tests.  It
/// intentionally implements the line framing expected by the runtime actor,
/// and never starts a shell or opens a network connection.
fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "legacy".into());
    let marker =
        env::args().find_map(|argument| argument.strip_prefix("--marker=").map(str::to_owned));
    let stderr_marker =
        env::args().find_map(|argument| argument.strip_prefix("--stderr=").map(str::to_owned));
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout());
    let mut call_number = 0usize;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            break;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let id = request.get("id").cloned();
        let Some(id) = id else {
            // notifications/initialized have no response.
            continue;
        };
        if let Some(stderr_marker) = stderr_marker.as_deref() {
            eprint!("{stderr_marker}");
            let _ = io::stderr().flush();
        }
        if mode == "protocol-error" {
            // Deliberately violate JSON-lines framing. The broker must map
            // this to a stable protocol failure and never expose the bytes.
            let _ = stdout.write_all(b"not-json\n");
            let _ = stdout.flush();
            break;
        }
        if mode == "legacy-restart"
            && method == "server/discover"
            && let Some(marker) = marker.as_deref()
            && !std::path::Path::new(marker).exists()
        {
            std::fs::write(marker, b"first-process").expect("write restart marker");
            // Simulate a target that closes the connection after the modern
            // probe. The actor must spawn once and send legacy initialize as
            // the first message to the replacement.
            break;
        }
        if mode == "legacy-restart"
            && let Some(marker) = marker.as_deref()
        {
            let event = match method {
                "initialize" => Some("initialize-restart\n"),
                "tools/call" => Some("tools-call\n"),
                _ => None,
            };
            if let Some(event) = event {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(marker)
                    .expect("open restart marker");
                file.write_all(event.as_bytes())
                    .expect("record restart event");
            }
        }
        if mode == "stderr-overflow" {
            eprint!("{}", "STDERR_SENTINEL".repeat(512));
            let _ = io::stderr().flush();
        }
        let response = match (mode.as_str(), method) {
            (mode, "server/discover") if mode.starts_with("wire-padding-") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"protocolVersion":"2026-07-28","capabilities":{"tools":{}}}
            }),
            (mode, "tools/call") if mode.starts_with("wire-padding-") => {
                call_result(&request, false)
            }
            ("modern", "server/discover") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"protocolVersion":"2026-07-28","capabilities":{"tools":{}}}
            }),
            ("multiple-text", "server/discover") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"protocolVersion":"2026-07-28","capabilities":{"tools":{}}}
            }),
            ("legacy", "server/discover") | ("bad-content", "server/discover") => json!({
                "jsonrpc":"2.0", "id":id,
                "error":{"code":-32601,"message":"method not found"}
            }),
            ("modern", "tools/call")
            | ("legacy", "tools/call")
            | ("legacy-restart", "tools/call")
            | ("catalog-noise", "tools/call") => call_result(&request, false),
            ("reuse", "tools/call") => {
                call_number += 1;
                call_result_with_number(&request, call_number)
            }
            ("slow", "tools/call") => {
                thread::sleep(Duration::from_millis(250));
                call_result(&request, false)
            }
            ("timeout", "tools/call") => {
                thread::sleep(Duration::from_secs(30));
                call_result(&request, false)
            }
            ("reuse", "server/discover") | ("slow", "server/discover") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"protocolVersion":"2026-07-28","capabilities":{"tools":{}}}
            }),
            ("structured", "server/discover") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"protocolVersion":"2026-07-28","capabilities":{"tools":{}}}
            }),
            ("structured", "tools/call") => call_result(&request, true),
            ("multiple-text", "tools/call") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"content":[
                    {"type":"text","text":"first"},
                    {"type":"text","text":"second"},
                    {"type":"text","text":"third"}
                ],"isError":false}
            }),
            ("catalog-noise", "tools/list") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"tools":[
                    {"name":"public-must-not-appear","inputSchema":{"type":"object"}},
                    {"name":"echo_arguments","inputSchema":{"type":"object"}}
                ]}
            }),
            ("bad-content", "tools/call") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"content":[{"type":"image","data":"not-forwarded"}],"isError":false}
            }),
            ("mixed-content", "tools/call") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"content":[
                    {"type":"text","text":"not-forwarded"},
                    {"type":"image","data":"not-forwarded"}
                ],"isError":false}
            }),
            ("server-request", "tools/call") => json!({
                "jsonrpc":"2.0", "id":id,
                "method":"sampling/createMessage",
                "params":{"secret":"not-forwarded"}
            }),
            ("input-required", "tools/call") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"content":[{"type":"text","text":"input required"}],"isError":true}
            }),
            (_, "initialize") => json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}
            }),
            _ => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}})
            }
        };
        let mut encoded = match serde_json::to_vec(&response) {
            Ok(encoded) => encoded,
            Err(_) => break,
        };
        if mode.starts_with("wire-padding-") && method == "tools/call" {
            let target = mode
                .strip_prefix("wire-padding-")
                .and_then(|value| value.parse::<usize>().ok())
                .expect("wire-padding fixture size");
            if encoded.len() > target {
                break;
            }
            encoded.resize(target, b' ');
        }
        if stdout.write_all(&encoded).is_err()
            || stdout.write_all(b"\n").is_err()
            || stdout.flush().is_err()
        {
            break;
        }
    }
}

fn call_result(request: &Value, structured: bool) -> Value {
    let arguments = request
        .get("params")
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let text = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".into());
    if structured {
        json!({
            "jsonrpc":"2.0", "id":request.get("id"),
            "result":{"content":[{"type":"text","text":text}],"structuredContent":{"received":arguments},"isError":false,"_meta":{"private":true}}
        })
    } else {
        json!({
            "jsonrpc":"2.0", "id":request.get("id"),
            "result":{"content":[{"type":"text","text":text}],"isError":false,"_meta":{"private":true}}
        })
    }
}

fn call_result_with_number(request: &Value, call_number: usize) -> Value {
    let arguments = request
        .get("params")
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let text = serde_json::to_string(&json!({
        "arguments": arguments,
        "callNumber": call_number,
    }))
    .unwrap_or_else(|_| "{}".into());
    json!({
        "jsonrpc":"2.0", "id":request.get("id"),
        "result":{"content":[{"type":"text","text":text}],"isError":false,"_meta":{"private":true}}
    })
}
