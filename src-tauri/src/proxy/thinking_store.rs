//! Server-side full thinking-block store.
//!
//! Captures untruncated thought text + thoughtSignature from upstream Gemini
//! responses, then precisely re-injects them into the next request's `contents`
//! even when the client dropped / truncated thinking.
//!
//! Matching is content-based (visible assistant text + tool ids), not turn
//! index, so OpenAI / Anthropic / Gemini packet shapes can differ.
//!
//! Isolation key = `{tenant}:{client_session_id}`:
//! - tenant is a hash of the caller's API key
//! - client_session_id prefers `X-Session-Id` / body `session_id`

use axum::http::HeaderMap;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MIN_SIGNATURE_LENGTH: usize = 50;
const SENTINEL_SIGNATURE: &str = "skip_thought_signature_validator";
const MAX_SESSIONS: usize = 2000;
const MAX_TURNS_PER_SESSION: usize = 200;
const MAX_BYTES_PER_SESSION: usize = 32 * 1024 * 1024;
const IDLE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

const PLACEHOLDER_THOUGHTS: &[&str] = &[
    "...",
    "[undefined]",
    "Applying tool decisions and generating response...",
];

/// Server-side auto thinking. Clients usually omit thinking config;
/// if any of `gemini` / `flash` / `pro` / `agent` appears in a model id
/// (requested or mapped), the proxy enables thoughts + signature restore.
/// Image / lite / preview are excluded to avoid 400s.
pub fn model_forces_server_thinking(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.is_empty() {
        return false;
    }
    if m.contains("image") || m.contains("lite") || m.contains("preview") {
        return false;
    }
    m.contains("gemini") || m.contains("flash") || m.contains("pro") || m.contains("agent")
}

pub fn any_model_forces_server_thinking(models: &[&str]) -> bool {
    models.iter().copied().any(model_forces_server_thinking)
}

#[derive(Debug, Clone)]
pub struct ThinkingRecord {
    pub fingerprint: String,
    pub thought: String,
    pub signature: Option<String>,
    pub tool_ids: Vec<String>,
    #[allow(dead_code)]
    pub tool_names: Vec<String>,
    pub visible: String,
}

#[derive(Debug)]
struct SessionEntry {
    turns: Vec<ThinkingRecord>,
    last_access: Instant,
    bytes: usize,
}

impl SessionEntry {
    fn new() -> Self {
        Self {
            turns: Vec::new(),
            last_access: Instant::now(),
            bytes: 0,
        }
    }
}

pub struct ThinkingStore {
    sessions: Mutex<HashMap<String, SessionEntry>>,
}

impl ThinkingStore {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn global() -> &'static ThinkingStore {
        static INSTANCE: OnceLock<ThinkingStore> = OnceLock::new();
        INSTANCE.get_or_init(ThinkingStore::new)
    }

    pub fn record(&self, store_key: &str, rec: ThinkingRecord) {
        if rec.thought.trim().is_empty() && rec.signature.is_none() {
            return;
        }
        let rec_bytes = rec.thought.len()
            + rec.signature.as_ref().map(|s| s.len()).unwrap_or(0)
            + rec.visible.len();

        let Ok(mut map) = self.sessions.lock() else {
            return;
        };
        evict_idle_locked(&mut map);

        let entry = map
            .entry(store_key.to_string())
            .or_insert_with(SessionEntry::new);
        entry.last_access = Instant::now();

        if let Some(last) = entry.turns.last_mut() {
            if last.fingerprint == rec.fingerprint {
                if rec.thought.len() >= last.thought.len()
                    || rec.signature.as_ref().map(|s| s.len()).unwrap_or(0)
                        > last.signature.as_ref().map(|s| s.len()).unwrap_or(0)
                {
                    entry.bytes = entry.bytes.saturating_sub(last.thought.len() + last.visible.len());
                    *last = rec;
                    entry.bytes = entry.bytes.saturating_add(rec_bytes);
                }
                return;
            }
        }

        entry.turns.push(rec);
        entry.bytes = entry.bytes.saturating_add(rec_bytes);

        while entry.turns.len() > MAX_TURNS_PER_SESSION || entry.bytes > MAX_BYTES_PER_SESSION {
            if let Some(old) = entry.turns.first() {
                let old_bytes = old.thought.len()
                    + old.signature.as_ref().map(|s| s.len()).unwrap_or(0)
                    + old.visible.len();
                entry.bytes = entry.bytes.saturating_sub(old_bytes);
            }
            if entry.turns.is_empty() {
                break;
            }
            entry.turns.remove(0);
        }

        if map.len() > MAX_SESSIONS {
            if let Some(oldest_key) = map
                .iter()
                .min_by_key(|(_, v)| v.last_access)
                .map(|(k, _)| k.clone())
            {
                if oldest_key != store_key {
                    map.remove(&oldest_key);
                }
            }
        }
    }

    pub fn restore_gemini_contents(&self, store_key: &str, contents: &mut Vec<Value>) -> usize {
        if store_key.is_empty() {
            return 0;
        }
        let records = {
            let Ok(mut map) = self.sessions.lock() else {
                return 0;
            };
            let Some(entry) = map.get_mut(store_key) else {
                return 0;
            };
            entry.last_access = Instant::now();
            entry.turns.clone()
        };
        if records.is_empty() {
            return 0;
        }

        let mut used = vec![false; records.len()];
        let mut restored = 0usize;

        for content in contents.iter_mut() {
            let role = content
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if role != "model" && role != "assistant" {
                continue;
            }
            let Some(parts) = content.get_mut("parts").and_then(|p| p.as_array_mut()) else {
                continue;
            };

            let (visible, tool_ids, tool_names, existing_thought) = inspect_parts(parts);
            let fp = fingerprint(&visible, &tool_ids, &tool_names);

            let match_idx = find_record(&records, &used, &fp, &tool_ids);
            let Some(idx) = match_idx else {
                continue;
            };
            used[idx] = true;
            let rec = &records[idx];
            if rec.thought.trim().is_empty() {
                continue;
            }

            let should_replace = is_placeholder_thought(&existing_thought)
                || existing_thought.len() < rec.thought.len()
                || (rec.signature.is_some() && !parts.iter().any(|p| part_has_signature(p)));

            if !should_replace {
                continue;
            }

            parts.retain(|p| p.get("thought").and_then(|t| t.as_bool()) != Some(true));

            let mut thought_part = json!({
                "text": rec.thought,
                "thought": true,
            });
            if let Some(sig) = rec.signature.as_ref().filter(|s| is_real_signature(s)) {
                thought_part["thoughtSignature"] = json!(sig);
                thought_part["thought_signature"] = json!(sig);
                for part in parts.iter_mut() {
                    if part.get("functionCall").is_some() && !part_has_signature(part) {
                        part["thoughtSignature"] = json!(sig);
                        part["thought_signature"] = json!(sig);
                    }
                }
            }
            parts.insert(0, thought_part);
            restored += 1;
        }

        if restored > 0 {
            tracing::info!(
                "[ThinkingStore] Restored {} full thinking block(s) for session {}",
                restored,
                store_key
            );
        }
        restored
    }

    pub fn end_session(&self, store_key: &str) -> EndSessionResult {
        let Ok(mut map) = self.sessions.lock() else {
            return EndSessionResult {
                session_id: client_id_from_store_key(store_key).to_string(),
                deleted_turns: 0,
                deleted_bytes: 0,
            };
        };
        let removed = map.remove(store_key);
        let (deleted_turns, deleted_bytes) = removed
            .map(|e| (e.turns.len(), e.bytes))
            .unwrap_or((0, 0));
        EndSessionResult {
            session_id: client_id_from_store_key(store_key).to_string(),
            deleted_turns,
            deleted_bytes,
        }
    }

    pub fn session_stats(&self, store_key: &str) -> Option<(usize, usize)> {
        let Ok(map) = self.sessions.lock() else {
            return None;
        };
        map.get(store_key).map(|e| (e.turns.len(), e.bytes))
    }

    #[cfg(test)]
    pub fn clear(&self) {
        if let Ok(mut map) = self.sessions.lock() {
            map.clear();
        }
    }
}

#[derive(Debug, Clone)]
pub struct EndSessionResult {
    pub session_id: String,
    pub deleted_turns: usize,
    pub deleted_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct SessionScope {
    pub client_id: String,
    pub store_key: String,
}

impl SessionScope {
    pub fn from_headers(headers: &HeaderMap, fallback: impl Into<String>) -> Self {
        let fallback = fallback.into();
        let client_id = explicit_session_id(headers, None)
            .unwrap_or(fallback)
            .trim()
            .to_string();
        let client_id = sanitize_session_id(&client_id);
        let tenant = tenant_from_headers(headers);
        let store_key = format!("{}:{}", tenant, client_id);
        Self {
            client_id,
            store_key,
        }
    }

    #[allow(dead_code)]
    pub fn from_headers_and_body(
        headers: &HeaderMap,
        body: Option<&Value>,
        fallback: impl Into<String>,
    ) -> Self {
        let fallback = fallback.into();
        let client_id = explicit_session_id(headers, body)
            .unwrap_or(fallback)
            .trim()
            .to_string();
        let client_id = sanitize_session_id(&client_id);
        let tenant = tenant_from_headers(headers);
        let store_key = format!("{}:{}", tenant, client_id);
        Self {
            client_id,
            store_key,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TurnAccumulator {
    thought: String,
    signature: Option<String>,
    visible: String,
    tool_ids: Vec<String>,
    tool_names: Vec<String>,
}

impl TurnAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest_part(&mut self, part: &Value) {
        let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            if is_thought {
                self.thought.push_str(text);
            } else {
                self.visible.push_str(text);
            }
        }
        if let Some(sig) = part
            .get("thoughtSignature")
            .or_else(|| part.get("thought_signature"))
            .and_then(|s| s.as_str())
        {
            if is_real_signature(sig)
                && self
                    .signature
                    .as_ref()
                    .map(|old| sig.len() > old.len())
                    .unwrap_or(true)
            {
                self.signature = Some(sig.to_string());
            }
        }
        if let Some(fc) = part.get("functionCall") {
            let name = fc
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let id = fc
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("call_{}_{}", name, self.tool_ids.len()));
            if !self.tool_ids.iter().any(|x| x == &id) {
                self.tool_ids.push(id);
                self.tool_names.push(name);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.thought.trim().is_empty() && self.signature.is_none()
    }

    pub fn commit(self, store_key: &str) {
        if self.is_empty() || store_key.is_empty() {
            return;
        }
        let fp = fingerprint(&self.visible, &self.tool_ids, &self.tool_names);
        tracing::debug!(
            "[ThinkingStore] Capture thought len={} sig_len={} fp={} sid={}",
            self.thought.len(),
            self.signature.as_ref().map(|s| s.len()).unwrap_or(0),
            fp,
            store_key
        );
        ThinkingStore::global().record(
            store_key,
            ThinkingRecord {
                fingerprint: fp,
                thought: self.thought,
                signature: self.signature,
                tool_ids: self.tool_ids,
                tool_names: self.tool_names,
                visible: self.visible,
            },
        );
    }
}

pub fn capture_gemini_parts(store_key: &str, parts: &[Value]) {
    let mut acc = TurnAccumulator::new();
    for part in parts {
        acc.ingest_part(part);
    }
    acc.commit(store_key);
}

pub fn capture_gemini_response(store_key: &str, response: &Value) {
    let raw = response.get("response").unwrap_or(response);
    if let Some(parts) = raw
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        capture_gemini_parts(store_key, parts);
    }
}

fn explicit_session_id(headers: &HeaderMap, body: Option<&Value>) -> Option<String> {
    for name in ["x-session-id", "x-antigravity-session-id"] {
        if let Some(v) = headers.get(name).and_then(|h| h.to_str().ok()) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    if let Some(body) = body {
        if let Some(v) = body.get("session_id").and_then(|v| v.as_str()) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
        if let Some(v) = body
            .get("metadata")
            .and_then(|m| m.get("user_id"))
            .and_then(|v| v.as_str())
        {
            let v = v.trim();
            if !v.is_empty() && !v.contains("session-") {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn tenant_from_headers(headers: &HeaderMap) -> String {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or(Some(s)))
        .or_else(|| headers.get("x-api-key").and_then(|h| h.to_str().ok()))
        .or_else(|| headers.get("x-goog-api-key").and_then(|h| h.to_str().ok()))
        .unwrap_or("anon");
    let hash = format!("{:x}", Sha256::digest(raw.as_bytes()));
    hash[..16].to_string()
}

pub fn sanitize_session_id(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars().take(128) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':') {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "sid-unknown".to_string()
    } else {
        out
    }
}

fn client_id_from_store_key(store_key: &str) -> &str {
    store_key.split_once(':').map(|(_, rest)| rest).unwrap_or(store_key)
}

fn is_real_signature(sig: &str) -> bool {
    sig.len() >= MIN_SIGNATURE_LENGTH && sig != SENTINEL_SIGNATURE
}

fn is_placeholder_thought(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || PLACEHOLDER_THOUGHTS.contains(&t)
}

fn part_has_signature(part: &Value) -> bool {
    part.get("thoughtSignature")
        .or_else(|| part.get("thought_signature"))
        .and_then(|s| s.as_str())
        .is_some_and(is_real_signature)
}

fn inspect_parts(parts: &[Value]) -> (String, Vec<String>, Vec<String>, String) {
    let mut visible = String::new();
    let mut thought = String::new();
    let mut tool_ids = Vec::new();
    let mut tool_names = Vec::new();
    for part in parts {
        let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            if is_thought {
                thought.push_str(text);
            } else {
                visible.push_str(text);
            }
        }
        if let Some(fc) = part.get("functionCall") {
            let name = fc
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let id = fc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !id.is_empty() {
                tool_ids.push(id);
            }
            tool_names.push(name);
        }
    }
    (visible, tool_ids, tool_names, thought)
}

pub fn fingerprint(visible: &str, tool_ids: &[String], tool_names: &[String]) -> String {
    let mut hasher = Sha256::new();
    let norm: String = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    hasher.update(norm.as_bytes());
    hasher.update([0xff]);
    for id in tool_ids {
        hasher.update(id.as_bytes());
        hasher.update([0xfe]);
    }
    hasher.update([0xfd]);
    for name in tool_names {
        hasher.update(name.as_bytes());
        hasher.update([0xfc]);
    }
    let hex = format!("{:x}", hasher.finalize());
    hex[..16].to_string()
}

fn find_record(
    records: &[ThinkingRecord],
    used: &[bool],
    fp: &str,
    tool_ids: &[String],
) -> Option<usize> {
    for (i, rec) in records.iter().enumerate() {
        if used[i] {
            continue;
        }
        if rec.fingerprint == fp {
            return Some(i);
        }
    }
    if !tool_ids.is_empty() {
        for (i, rec) in records.iter().enumerate() {
            if used[i] {
                continue;
            }
            if rec.tool_ids.iter().any(|id| tool_ids.iter().any(|t| t == id)) {
                return Some(i);
            }
        }
    }
    None
}

fn evict_idle_locked(map: &mut HashMap<String, SessionEntry>) {
    map.retain(|_, e| e.last_access.elapsed() < IDLE_TTL);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(thought: &str, visible: &str, tool_id: Option<&str>) -> ThinkingRecord {
        let tool_ids = tool_id
            .map(|id| vec![id.to_string()])
            .unwrap_or_default();
        let tool_names = if tool_id.is_some() {
            vec!["shell".to_string()]
        } else {
            Vec::new()
        };
        let fp = fingerprint(visible, &tool_ids, &tool_names);
        ThinkingRecord {
            fingerprint: fp,
            thought: thought.to_string(),
            signature: Some("s".repeat(60)),
            tool_ids,
            tool_names,
            visible: visible.to_string(),
        }
    }

    #[test]
    fn stores_full_thought_without_truncation() {
        let store = ThinkingStore::new();
        let long = "T".repeat(50_000);
        store.record("t:s1", rec(&long, "hello world", None));
        let mut contents = vec![json!({
            "role": "model",
            "parts": [{ "text": "hello world" }]
        })];
        let n = store.restore_gemini_contents("t:s1", &mut contents);
        assert_eq!(n, 1);
        assert_eq!(contents[0]["parts"][0]["text"].as_str().unwrap().len(), 50_000);
        assert_eq!(contents[0]["parts"][0]["thought"], true);
        assert_eq!(
            contents[0]["parts"][0]["thoughtSignature"].as_str().unwrap().len(),
            60
        );
    }

    #[test]
    fn matches_by_visible_text_not_index() {
        let store = ThinkingStore::new();
        store.record("t:s1", rec("think-A", "answer A", None));
        store.record("t:s1", rec("think-B", "answer B", None));

        // Client dropped turn A, only sends B (different packet shape / rewind)
        let mut contents = vec![json!({
            "role": "model",
            "parts": [{ "text": "answer B" }]
        })];
        store.restore_gemini_contents("t:s1", &mut contents);
        assert_eq!(contents[0]["parts"][0]["text"], "think-B");
    }

    #[test]
    fn matches_tool_id_when_text_missing() {
        let store = ThinkingStore::new();
        store.record("t:s1", rec("plan", "", Some("call_1")));
        let mut contents = vec![json!({
            "role": "model",
            "parts": [{
                "functionCall": { "name": "shell", "id": "call_1", "args": {} }
            }]
        })];
        store.restore_gemini_contents("t:s1", &mut contents);
        assert_eq!(contents[0]["parts"][0]["thought"], true);
        assert_eq!(contents[0]["parts"][0]["text"], "plan");
        assert_eq!(
            contents[0]["parts"][1]["thoughtSignature"].as_str().unwrap().len(),
            60
        );
    }

    #[test]
    fn replaces_placeholder_dots() {
        let store = ThinkingStore::new();
        store.record("t:s1", rec("full chain", "final", None));
        let mut contents = vec![json!({
            "role": "model",
            "parts": [
                { "text": "...", "thought": true },
                { "text": "final" }
            ]
        })];
        store.restore_gemini_contents("t:s1", &mut contents);
        assert_eq!(contents[0]["parts"][0]["text"], "full chain");
    }

    #[test]
    fn tenant_isolation_and_end_session() {
        let store = ThinkingStore::new();
        store.record("aaa:chat", rec("secret-a", "hi", None));
        store.record("bbb:chat", rec("secret-b", "hi", None));

        let mut a = vec![json!({"role":"model","parts":[{"text":"hi"}]})];
        store.restore_gemini_contents("aaa:chat", &mut a);
        assert_eq!(a[0]["parts"][0]["text"], "secret-a");

        let result = store.end_session("aaa:chat");
        assert_eq!(result.deleted_turns, 1);
        let mut a2 = vec![json!({"role":"model","parts":[{"text":"hi"}]})];
        assert_eq!(store.restore_gemini_contents("aaa:chat", &mut a2), 0);

        let mut b = vec![json!({"role":"model","parts":[{"text":"hi"}]})];
        store.restore_gemini_contents("bbb:chat", &mut b);
        assert_eq!(b[0]["parts"][0]["text"], "secret-b");
    }

    #[test]
    fn sanitize_rejects_junk() {
        assert_eq!(sanitize_session_id("abc/../x"), "abc..x");
        assert_eq!(sanitize_session_id(""), "sid-unknown");
    }

    #[test]
    fn keyword_forces_thinking_without_client_flag() {
        assert!(model_forces_server_thinking("gemini-3-flash"));
        assert!(model_forces_server_thinking("gemini-3-pro"));
        assert!(model_forces_server_thinking("gemini-3-flash-agent"));
        assert!(model_forces_server_thinking("gemini-pro-agent"));
        assert!(any_model_forces_server_thinking(&["claude-sonnet-4-6", "gemini-3-flash"]));
        assert!(!model_forces_server_thinking("claude-sonnet-4-6"));
        assert!(!model_forces_server_thinking("gemini-3-pro-image"));
        assert!(!model_forces_server_thinking("gemini-3.1-flash-lite"));
        assert!(!model_forces_server_thinking("gemini-3-pro-preview"));
    }

    #[test]
    fn same_fingerprint_updates_in_place() {
        let store = ThinkingStore::new();
        store.record("t:s1", rec("short", "same", None));
        store.record("t:s1", rec("much longer thought", "same", None));
        let stats = store.session_stats("t:s1").unwrap();
        assert_eq!(stats.0, 1);
        let mut contents = vec![json!({"role":"model","parts":[{"text":"same"}]})];
        store.restore_gemini_contents("t:s1", &mut contents);
        assert_eq!(contents[0]["parts"][0]["text"], "much longer thought");
    }
}
