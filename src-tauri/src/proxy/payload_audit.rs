//! Request/response payload audit helpers.
//!
//! - Header serialization with API-key redaction
//! - Session / thinking markers are preserved for ops comparison
//! - Optional concise storage mode to keep SQLite small

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};

const MAX_SIMPLE_STRING: usize = 800;
const MAX_SIMPLE_SIGNATURE: usize = 48;
const MAX_SIMPLE_SYSTEM: usize = 1500;

const THINKING_SESSION_HEADERS: &[&str] = &[
    "x-session-id",
    "x-antigravity-session-id",
    "x-conversation-id",
    "conversation-id",
    "x-chat-id",
    "chat-id",
    "x-thread-id",
    "thread-id",
    "x-client-session-id",
    "x-cursor-session-id",
    "cursor-session-id",
    "x-vscode-session-id",
    "anthropic-session-id",
    "mcp-session-id",
];

fn is_thinking_session_header(name: &str) -> bool {
    THINKING_SESSION_HEADERS
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h))
}

fn is_sensitive_header(name: &str) -> bool {
    if is_thinking_session_header(name) {
        return false;
    }
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "authorization"
            | "proxy-authorization"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
            | "anthropic-api-key"
            | "x-auth-token"
            | "x-access-token"
            | "cookie"
            | "set-cookie"
    ) || n.contains("api-key")
        || n.contains("apikey")
        || n.contains("access-token")
        || n.contains("access_token")
        || (n.contains("token") && !n.contains("session") && !n.contains("count"))
}

pub fn redact_header_value(name: &str, value: &str) -> String {
    if is_thinking_session_header(name) {
        return value.to_string();
    }
    if !is_sensitive_header(name) {
        return value.to_string();
    }
    let trimmed = value.trim();
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
        return "Bearer ***REDACTED***".to_string();
    }
    "***REDACTED***".to_string()
}

pub fn headers_to_redacted_json(headers: &HeaderMap) -> String {
    header_pairs_to_redacted_json(headers.iter().filter_map(|(k, v)| {
        v.to_str().ok().map(|s| (k.as_str(), s))
    }))
}

pub fn header_pairs_to_redacted_json<'a, I>(pairs: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut map = Map::new();
    for (key, raw) in pairs {
        let redacted = redact_header_value(key, raw);
        match map.get_mut(key) {
            Some(Value::Array(arr)) => arr.push(json!(redacted)),
            Some(existing) => {
                let prev = existing.clone();
                *existing = json!([prev, redacted]);
            }
            None => {
                map.insert(key.to_string(), json!(redacted));
            }
        }
    }
    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| "{}".to_string())
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

fn simplify_string(s: &str, max: usize) -> Value {
    json!(truncate_chars(s, max))
}

fn simplify_part(part: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(thought) = part.get("thought") {
        obj.insert("thought".into(), thought.clone());
    }
    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
        let max = if part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false) {
            400
        } else {
            MAX_SIMPLE_STRING
        };
        obj.insert("text".into(), simplify_string(text, max));
    }
    for sig_key in ["thoughtSignature", "thought_signature", "signature"] {
        if let Some(sig) = part.get(sig_key).and_then(|s| s.as_str()) {
            obj.insert(sig_key.to_string(), simplify_string(sig, MAX_SIMPLE_SIGNATURE));
        }
    }
    if let Some(fc) = part.get("functionCall") {
        let mut fc_out = Map::new();
        if let Some(name) = fc.get("name") {
            fc_out.insert("name".into(), name.clone());
        }
        if let Some(id) = fc.get("id") {
            fc_out.insert("id".into(), id.clone());
        }
        if fc.get("args").is_some() {
            fc_out.insert("args".into(), json!("[omitted]"));
        }
        obj.insert("functionCall".into(), Value::Object(fc_out));
    }
    if let Some(fr) = part.get("functionResponse") {
        let mut fr_out = Map::new();
        if let Some(name) = fr.get("name") {
            fr_out.insert("name".into(), name.clone());
        }
        if let Some(id) = fr.get("id") {
            fr_out.insert("id".into(), id.clone());
        }
        fr_out.insert("response".into(), json!("[omitted]"));
        obj.insert("functionResponse".into(), Value::Object(fr_out));
    }
    if let Some(inline) = part.get("inlineData") {
        obj.insert(
            "inlineData".into(),
            json!({
                "mimeType": inline.get("mimeType").cloned().unwrap_or(json!("unknown")),
                "data": format!("[omitted: {} chars]", inline.get("data").and_then(|d| d.as_str()).map(|s| s.len()).unwrap_or(0)),
            }),
        );
    }
    if obj.is_empty() {
        return json!("[omitted]");
    }
    Value::Object(obj)
}

fn simplify_message(msg: &Value) -> Value {
    let mut out = Map::new();
    if let Some(role) = msg.get("role") {
        out.insert("role".into(), role.clone());
    }
    if let Some(parts) = msg.get("parts").and_then(|p| p.as_array()) {
        out.insert(
            "parts".into(),
            Value::Array(parts.iter().map(simplify_part).collect()),
        );
    }
    if let Some(content) = msg.get("content") {
        out.insert("content".into(), simplify_content(content));
    }
    if let Some(thinking) = msg.get("thinking") {
        out.insert("thinking".into(), thinking.clone());
    }
    if let Some(rc) = msg.get("reasoning_content").and_then(|s| s.as_str()) {
        out.insert("reasoning_content".into(), simplify_string(rc, 400));
    }
    if let Some(sig) = msg.get("thinking_signature").or_else(|| msg.get("signature")) {
        if let Some(s) = sig.as_str() {
            out.insert("signature".into(), simplify_string(s, MAX_SIMPLE_SIGNATURE));
        }
    }
    if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        out.insert(
            "tool_calls".into(),
            json!(tool_calls
                .iter()
                .map(|tc| {
                    json!({
                        "id": tc.get("id"),
                        "type": tc.get("type"),
                        "function": {
                            "name": tc.get("function").and_then(|f| f.get("name")),
                            "arguments": "[omitted]"
                        }
                    })
                })
                .collect::<Vec<_>>()),
        );
    }
    Value::Object(out)
}

fn simplify_content(content: &Value) -> Value {
    match content {
        Value::String(s) => simplify_string(s, MAX_SIMPLE_STRING),
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|block| {
                    if let Some(obj) = block.as_object() {
                        let mut slim = Map::new();
                        if let Some(t) = obj.get("type") {
                            slim.insert("type".into(), t.clone());
                        }
                        if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
                            slim.insert("text".into(), simplify_string(text, MAX_SIMPLE_STRING));
                        }
                        if let Some(thinking) = obj.get("thinking").and_then(|t| t.as_str()) {
                            slim.insert("thinking".into(), simplify_string(thinking, 400));
                        }
                        for sig_key in ["signature", "thoughtSignature", "thinking_signature"] {
                            if let Some(sig) = obj.get(sig_key).and_then(|s| s.as_str()) {
                                slim.insert(sig_key.to_string(), simplify_string(sig, MAX_SIMPLE_SIGNATURE));
                            }
                        }
                        if let Some(id) = obj.get("id") {
                            slim.insert("id".into(), id.clone());
                        }
                        if let Some(name) = obj.get("name") {
                            slim.insert("name".into(), name.clone());
                        }
                        if obj.contains_key("input") {
                            slim.insert("input".into(), json!("[omitted]"));
                        }
                        Value::Object(slim)
                    } else {
                        json!("[omitted]")
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn simplify_tools(tools: &Value) -> Value {
    match tools {
        Value::Array(arr) => json!(arr
            .iter()
            .map(|t| t.get("name").or_else(|| t.get("function").and_then(|f| f.get("name"))).cloned().unwrap_or(json!("tool")))
            .collect::<Vec<_>>()),
        other => json!(format!("[tools omitted: {} chars]", other.to_string().len())),
    }
}

pub fn simplify_payload_json(value: &Value) -> Value {
    let inner = value.get("request").unwrap_or(value);
    let mut concise = Map::new();

    for key in [
        "model",
        "session_id",
        "conversation_id",
        "chat_id",
        "thread_id",
        "previous_response_id",
        "thinking",
        "reasoning_effort",
        "reasoning",
        "stream",
    ] {
        if let Some(v) = inner.get(key).or_else(|| value.get(key)) {
            concise.insert(key.to_string(), v.clone());
        }
    }

    if let Some(sys) = inner.get("system").or_else(|| value.get("system")) {
        match sys {
            Value::String(s) => {
                concise.insert("system".into(), simplify_string(s, MAX_SIMPLE_SYSTEM));
            }
            other => {
                concise.insert("system".into(), other.clone());
            }
        }
    }
    if let Some(sys) = inner
        .get("systemInstruction")
        .or_else(|| value.get("systemInstruction"))
    {
        concise.insert("systemInstruction".into(), sys.clone());
    }
    if let Some(cfg) = inner
        .get("generationConfig")
        .and_then(|g| g.get("thinkingConfig"))
    {
        concise.insert("thinkingConfig".into(), cfg.clone());
    }

    if let Some(messages) = inner.get("messages").or_else(|| value.get("messages")) {
        if let Some(arr) = messages.as_array() {
            concise.insert(
                "messages".into(),
                Value::Array(arr.iter().map(simplify_message).collect()),
            );
        }
    }
    if let Some(contents) = inner.get("contents").or_else(|| value.get("contents")) {
        if let Some(arr) = contents.as_array() {
            concise.insert(
                "contents".into(),
                Value::Array(arr.iter().map(simplify_message).collect()),
            );
        }
    }
    if let Some(tools) = inner.get("tools").or_else(|| value.get("tools")) {
        concise.insert("tools".into(), simplify_tools(tools));
    }
    if let Some(usage) = inner.get("usage").or_else(|| value.get("usage")) {
        concise.insert("usage".into(), usage.clone());
    }
    if let Some(usage) = inner
        .get("usageMetadata")
        .or_else(|| value.get("usageMetadata"))
    {
        concise.insert("usageMetadata".into(), usage.clone());
    }
    if let Some(choices) = value.get("choices") {
        concise.insert("choices".into(), choices.clone());
    }
    if let Some(candidates) = inner
        .get("candidates")
        .or_else(|| value.get("candidates"))
    {
        concise.insert("candidates".into(), candidates.clone());
    }

    if concise.is_empty() {
        return simplify_unknown(value);
    }
    Value::Object(concise)
}

fn simplify_unknown(value: &Value) -> Value {
    match value {
        Value::String(s) => simplify_string(s, MAX_SIMPLE_STRING),
        Value::Array(arr) => Value::Array(arr.iter().take(20).map(simplify_unknown).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map.iter().take(40) {
                out.insert(k.clone(), simplify_unknown(v));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub fn apply_storage_mode_to_body(raw: Option<String>, mode: &str) -> Option<String> {
    let raw = raw?;
    if mode != "simple" {
        return Some(raw);
    }
    match serde_json::from_str::<Value>(&raw) {
        Ok(json) => serde_json::to_string(&simplify_payload_json(&json)).ok().or(Some(raw)),
        Err(_) => Some(truncate_chars(&raw, 8000)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn redacts_api_keys_but_keeps_session_markers() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer sk-secret-customer-key"));
        headers.insert("x-api-key", HeaderValue::from_static("sk-another"));
        headers.insert("x-session-id", HeaderValue::from_static("sess-ops-compare-001"));
        headers.insert("x-antigravity-session-id", HeaderValue::from_static("ag-think-42"));
        headers.insert("user-agent", HeaderValue::from_static("claude-code/1.0"));

        let json = headers_to_redacted_json(&headers);
        assert!(json.contains("***REDACTED***"), "{json}");
        assert!(!json.contains("sk-secret-customer-key"), "{json}");
        assert!(!json.contains("sk-another"), "{json}");
        assert!(json.contains("sess-ops-compare-001"), "{json}");
        assert!(json.contains("ag-think-42"), "{json}");
        assert!(json.contains("claude-code/1.0"), "{json}");
    }
}
