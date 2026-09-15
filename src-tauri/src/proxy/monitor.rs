use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRequestLog {
    pub id: String,
    pub timestamp: i64,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration: u64,                // ms
    pub model: Option<String>,        // 客户端请求的模型名
    pub mapped_model: Option<String>, // 实际路由后使用的模型名
    pub account_email: Option<String>,
    pub client_ip: Option<String>, // 客户端 IP 地址
    pub error: Option<String>,
    pub request_body: Option<String>,
    pub upstream_request_body: Option<String>, // 网关转出给上游(Antigravity)的报文
    pub response_body: Option<String>,
    #[serde(default)]
    pub request_headers: Option<String>,
    #[serde(default)]
    pub upstream_request_headers: Option<String>,
    #[serde(default)]
    pub response_headers: Option<String>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cached_tokens: Option<u32>,
    pub protocol: Option<String>, // 协议类型: "openai", "anthropic", "gemini"
    pub username: Option<String>, // User token username
}

#[derive(Default)]
pub(crate) struct UpstreamCapture {
    body: Option<String>,
    headers: Option<String>,
}

#[derive(Clone, Default)]
pub struct UpstreamRequestBodyHolder(pub std::sync::Arc<std::sync::Mutex<UpstreamCapture>>);

tokio::task_local! {
    pub static CURRENT_UPSTREAM_CAPTURE: UpstreamRequestBodyHolder;
}

impl UpstreamRequestBodyHolder {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(UpstreamCapture::default())))
    }

    pub fn set(&self, body: String) {
        if let Ok(mut lock) = self.0.lock() {
            lock.body = Some(body);
        }
    }

    pub fn set_headers_json(&self, headers: String) {
        if let Ok(mut lock) = self.0.lock() {
            lock.headers = Some(headers);
        }
    }

    pub fn set_value(&self, val: &serde_json::Value) {
        let clean = sanitize_upstream_debug_value(val);
        if let Ok(s) = serde_json::to_string(&clean) {
            self.set(s);
        }
    }

    pub fn take(&self) -> Option<String> {
        self.0.lock().ok().and_then(|mut g| g.body.take())
    }

    pub fn take_headers(&self) -> Option<String> {
        self.0.lock().ok().and_then(|mut g| g.headers.take())
    }
}

fn sanitize_upstream_debug_value(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::String(s)
            if s.starts_with("data:image/") || s.starts_with("data:audio/") =>
        {
            serde_json::Value::String(format!("[inline data omitted: {} chars]", s.len()))
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sanitize_upstream_debug_value).collect())
        }
        serde_json::Value::Object(map) => {
            let is_inline = map.get("mimeType").and_then(serde_json::Value::as_str).is_some()
                && map.get("data").and_then(serde_json::Value::as_str).is_some();
            serde_json::Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        let v = if is_inline && k == "data" {
                            serde_json::Value::String(format!(
                                "[inline data omitted: {} chars]",
                                v.as_str().map(str::len).unwrap_or_default()
                            ))
                        } else {
                            sanitize_upstream_debug_value(v)
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            )
        }
        _ => val.clone(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyStats {
    pub total_requests: u64,
    pub success_count: u64,
    pub error_count: u64,
}

pub struct ProxyMonitor {
    pub logs: RwLock<VecDeque<ProxyRequestLog>>,
    pub stats: RwLock<ProxyStats>,
    pub max_logs: usize,
    pub enabled: AtomicBool,
    app_handle: Option<tauri::AppHandle>,
}

impl ProxyMonitor {
    pub fn new(max_logs: usize, app_handle: Option<tauri::AppHandle>) -> Self {
        // Initialize DB
        if let Err(e) = crate::modules::proxy_db::init_db() {
            tracing::error!("Failed to initialize proxy DB: {}", e);
        }

        let thinking_days = crate::proxy::config::get_thinking_retention_days() as i64;
        let retention = crate::modules::config::load_app_config()
            .map(|config| config.proxy.log_retention)
            .unwrap_or_default();
        tokio::task::spawn_blocking(move || {
            match crate::modules::proxy_db::apply_retention(&retention) {
                Ok((cleared, deleted)) => {
                    if cleared > 0 || deleted > 0 {
                        tracing::info!(
                            "Proxy log retention: cleared {} bodies, deleted {} rows",
                            cleared,
                            deleted
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to cleanup old logs: {}", e);
                }
            }
            match crate::modules::proxy_db::cleanup_old_thinking_records(thinking_days) {
                Ok(deleted) => {
                    if deleted > 0 {
                        tracing::info!(
                            "Auto cleanup: removed {} old thinking/signature records (>{} days)",
                            deleted,
                            thinking_days
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to cleanup thinking records: {}", e);
                }
            }
        });

        tokio::spawn(async {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await;
            loop {
                interval.tick().await;
                let thinking_days = crate::proxy::config::get_thinking_retention_days() as i64;
                let retention = crate::modules::config::load_app_config()
                    .map(|config| config.proxy.log_retention)
                    .unwrap_or_default();
                let result = tokio::task::spawn_blocking(move || {
                    let retention_res = crate::modules::proxy_db::apply_retention(&retention);
                    let thinking_res =
                        crate::modules::proxy_db::cleanup_old_thinking_records(thinking_days);
                    (retention_res, thinking_res)
                })
                .await;
                match result {
                    Ok((retention_res, thinking_res)) => {
                        match retention_res {
                            Ok((cleared, deleted)) => {
                                if cleared > 0 || deleted > 0 {
                                    tracing::info!(
                                        "Proxy log retention: cleared {} bodies, deleted {} rows",
                                        cleared,
                                        deleted
                                    );
                                }
                            }
                            Err(error) => {
                                tracing::error!("Failed to apply proxy log retention: {}", error)
                            }
                        }
                        if let Ok(deleted) = thinking_res {
                            if deleted > 0 {
                                tracing::info!(
                                    "Auto cleanup: removed {} old thinking/signature records",
                                    deleted
                                );
                            }
                        } else if let Err(e) = thinking_res {
                            tracing::error!("Failed to cleanup thinking records: {}", e);
                        }
                    }
                    Err(error) => tracing::error!("Proxy log retention task failed: {}", error),
                }
            }
        });

        Self {
            logs: RwLock::new(VecDeque::with_capacity(max_logs)),
            stats: RwLock::new(ProxyStats::default()),
            max_logs,
            enabled: AtomicBool::new(false), // Default to disabled
            app_handle,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub async fn log_request(&self, log: ProxyRequestLog) {
        if let (Some(account), Some(input), Some(output)) =
            (&log.account_email, log.input_tokens, log.output_tokens)
        {
            let model = log.model.clone().unwrap_or_else(|| "unknown".to_string());
            let account = account.clone();
            let cached = log.cached_tokens.unwrap_or(0);
            tokio::task::spawn_blocking(move || {
                if let Err(e) = crate::modules::token_stats::record_usage(
                    &account, &model, input, output, cached,
                ) {
                    tracing::debug!("Failed to record token stats: {}", e);
                }
            });
        }

        if !self.is_enabled() {
            return;
        }
        tracing::info!("[Monitor] Logging request: {} {}", log.method, log.url);
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_requests += 1;
            if log.status >= 200 && log.status < 400 {
                stats.success_count += 1;
            } else {
                stats.error_count += 1;
            }
        }

        // Add log to memory
        {
            let mut logs = self.logs.write().await;
            if logs.len() >= self.max_logs {
                logs.pop_back();
            }
            logs.push_front(log.clone());
        }

        // Save to DB (respect server-side simple/full storage mode)
        let mut log_to_save = log.clone();
        let storage_mode = crate::proxy::config::get_payload_storage_mode();
        log_to_save.request_body = crate::proxy::payload_audit::apply_storage_mode_to_body(
            log_to_save.request_body,
            &storage_mode,
        );
        log_to_save.upstream_request_body = crate::proxy::payload_audit::apply_storage_mode_to_body(
            log_to_save.upstream_request_body,
            &storage_mode,
        );
        log_to_save.response_body = crate::proxy::payload_audit::apply_storage_mode_to_body(
            log_to_save.response_body,
            &storage_mode,
        );
        tokio::task::spawn_blocking(move || {
            if let Err(e) = crate::modules::proxy_db::save_log(&log_to_save) {
                tracing::error!("Failed to save proxy log to DB: {}", e);
            }

            // Sync to Security DB (IpAccessLogs) so it appears in Security Monitor
            if let Some(ip) = &log_to_save.client_ip {
                let security_log = crate::modules::security_db::IpAccessLog {
                    id: uuid::Uuid::new_v4().to_string(),
                    client_ip: ip.clone(),
                    timestamp: log_to_save.timestamp / 1000, // ms to s
                    method: Some(log_to_save.method.clone()),
                    path: Some(log_to_save.url.clone()),
                    user_agent: None, // We don't have UA in ProxyRequestLog easily accessible here without plumbing
                    status: Some(log_to_save.status as i32),
                    duration: Some(log_to_save.duration as i64),
                    api_key_hash: None,
                    blocked: false, // This comes from monitor, so it wasn't blocked by IP filter
                    block_reason: None,
                    username: log_to_save.username.clone(),
                };

                if let Err(e) = crate::modules::security_db::save_ip_access_log(&security_log) {
                    tracing::error!("Failed to save security log: {}", e);
                }
            }
        });

        // Emit event (send summary only, without body to reduce memory)
        if let Some(app) = &self.app_handle {
            let log_summary = ProxyRequestLog {
                id: log.id.clone(),
                timestamp: log.timestamp,
                method: log.method.clone(),
                url: log.url.clone(),
                status: log.status,
                duration: log.duration,
                model: log.model.clone(),
                mapped_model: log.mapped_model.clone(),
                account_email: log.account_email.clone(),
                client_ip: log.client_ip.clone(),
                error: log.error.clone(),
                request_body: None,  // Don't send body in event
                upstream_request_body: None,
                response_body: None, // Don't send body in event
                request_headers: None,
                upstream_request_headers: None,
                response_headers: None,
                input_tokens: log.input_tokens,
                output_tokens: log.output_tokens,
                cached_tokens: log.cached_tokens,
                protocol: log.protocol.clone(),
                username: log.username.clone(),
            };
            let _ = app.emit("proxy://request", &log_summary);
        }
    }

    pub async fn get_logs(&self, limit: usize) -> Vec<ProxyRequestLog> {
        // Try to get from DB first for true history
        let db_result =
            tokio::task::spawn_blocking(move || crate::modules::proxy_db::get_logs(limit)).await;

        match db_result {
            Ok(Ok(logs)) => logs,
            Ok(Err(e)) => {
                tracing::error!("Failed to get logs from DB: {}", e);
                // Fallback to memory
                let logs = self.logs.read().await;
                logs.iter().take(limit).cloned().collect()
            }
            Err(e) => {
                tracing::error!("Spawn blocking failed for get_logs: {}", e);
                let logs = self.logs.read().await;
                logs.iter().take(limit).cloned().collect()
            }
        }
    }

    pub async fn get_stats(&self) -> ProxyStats {
        let db_result = tokio::task::spawn_blocking(|| crate::modules::proxy_db::get_stats()).await;

        match db_result {
            Ok(Ok(stats)) => stats,
            Ok(Err(e)) => {
                tracing::error!("Failed to get stats from DB: {}", e);
                self.stats.read().await.clone()
            }
            Err(e) => {
                tracing::error!("Spawn blocking failed for get_stats: {}", e);
                self.stats.read().await.clone()
            }
        }
    }

    pub async fn get_logs_filtered(
        &self,
        page: usize,
        page_size: usize,
        search_text: Option<String>,
        level: Option<String>,
    ) -> Result<Vec<ProxyRequestLog>, String> {
        let offset = (page.max(1) - 1) * page_size;
        let errors_only = level.as_deref() == Some("error");
        let search = search_text.unwrap_or_default();

        let res = tokio::task::spawn_blocking(move || {
            crate::modules::proxy_db::get_logs_filtered(&search, errors_only, page_size, offset)
        })
        .await;

        match res {
            Ok(r) => r,
            Err(e) => Err(format!("Spawn blocking failed: {}", e)),
        }
    }

    pub async fn clear(&self) {
        let mut logs = self.logs.write().await;
        logs.clear();
        let mut stats = self.stats.write().await;
        *stats = ProxyStats::default();

        let _ = tokio::task::spawn_blocking(|| {
            if let Err(e) = crate::modules::proxy_db::clear_logs() {
                tracing::error!("Failed to clear logs in DB: {}", e);
            }
        })
        .await;
    }
}
