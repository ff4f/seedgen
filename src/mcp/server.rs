use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::mcp::tools;

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// Standard JSON-RPC 2.0 error codes.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const SERVER_ERROR: i32 = -32000;

pub async fn run_stdio() -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = stdout;

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let msg = line.trim();
        if msg.is_empty() {
            continue;
        }

        if let Some(response) = handle_message(msg).await {
            writer.write_all(response.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

/// Returns `Some(json)` if a response is expected, `None` for notifications.
pub async fn handle_message(line: &str) -> Option<String> {
    let req: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Some(serialize_error(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            ));
        }
    };

    // Per JSON-RPC 2.0, a request without `id` is a notification; the server MUST NOT respond.
    let id = match req.id {
        Some(id) => id,
        None => return None,
    };

    let params = req.params.unwrap_or(Value::Null);
    let result = dispatch(&req.method, params).await;

    Some(match result {
        Ok(value) => serialize_success(id, value),
        Err((code, message)) => serialize_error(id, code, &message),
    })
}

async fn dispatch(method: &str, params: Value) -> Result<Value, (i32, String)> {
    match method {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(tools::tool_list()),
        "tools/call" => tools::dispatch_call(params).await,
        other => Err((METHOD_NOT_FOUND, format!("method not found: `{other}`"))),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "seedgen",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn serialize_success(id: Value, result: Value) -> String {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    };
    serde_json::to_string(&resp).expect("response is always serializable")
}

fn serialize_error(id: Value, code: i32, message: &str) -> String {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            data: None,
        }),
    };
    serde_json::to_string(&resp).expect("response is always serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).expect("response must be valid JSON")
    }

    #[tokio::test]
    async fn test_handle_initialize_returns_server_info() {
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let resp = handle_message(req).await.expect("expected response");
        let v = parse(&resp);
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["serverInfo"]["name"], "seedgen");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert!(v["result"]["protocolVersion"].is_string());
    }

    #[tokio::test]
    async fn test_handle_notification_returns_none() {
        // No `id` = notification; no response should be emitted.
        let req = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
        let resp = handle_message(req).await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn test_handle_unknown_method_returns_method_not_found() {
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"bogus","params":{}}"#;
        let resp = handle_message(req).await.expect("expected response");
        let v = parse(&resp);
        assert_eq!(v["id"], 2);
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
        assert!(v["error"]["message"].as_str().unwrap().contains("bogus"));
    }

    #[tokio::test]
    async fn test_handle_parse_error_returns_parse_error_code() {
        let resp = handle_message("not json").await.expect("expected response");
        let v = parse(&resp);
        assert_eq!(v["error"]["code"], PARSE_ERROR);
    }

    #[tokio::test]
    async fn test_handle_tools_list_returns_all_five_tools() {
        let req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#;
        let resp = handle_message(req).await.expect("expected response");
        let v = parse(&resp);
        let tools = v["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"seedgen_introspect"));
        assert!(names.contains(&"seedgen_generate"));
        assert!(names.contains(&"seedgen_reset"));
        assert!(names.contains(&"seedgen_list_scenarios"));
        assert!(names.contains(&"seedgen_validate"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_list_scenarios_works_without_db() {
        let req = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"seedgen_list_scenarios","arguments":{}}}"#;
        let resp = handle_message(req).await.expect("expected response");
        let v = parse(&resp);
        assert_eq!(v["id"], 4);
        let content = &v["result"]["content"];
        assert!(content.is_array());
        let text = content[0]["text"].as_str().unwrap();
        for name in ["ecommerce", "saas", "blog", "social"] {
            assert!(text.contains(name), "missing `{name}` in {text}");
        }
    }

    #[tokio::test]
    async fn test_handle_tools_call_unknown_tool_errors() {
        let req = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#;
        let resp = handle_message(req).await.expect("expected response");
        let v = parse(&resp);
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_preserves_string_ids() {
        let req = r#"{"jsonrpc":"2.0","id":"abc-123","method":"initialize","params":{}}"#;
        let resp = handle_message(req).await.expect("expected response");
        let v = parse(&resp);
        assert_eq!(v["id"], "abc-123");
    }
}
