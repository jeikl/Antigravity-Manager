use crate::proxy::config::LogRetentionConfig;
use crate::proxy::monitor::ProxyRequestLog;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, Connection};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

const THOUGHT_RAW_MAGIC: &[u8] = b"RAW1";
const THOUGHT_GZIP_MAGIC: &[u8] = b"AGZ1";
const MIN_GZIP_THOUGHT: usize = 384;
const SENTINEL_SIGNATURE: &str = "skip_thought_signature_validator";
const MIN_REAL_SIGNATURE: usize = 50;

fn persist_signature(signature: Option<&str>) -> Option<&str> {
    signature.filter(|s| s.len() >= MIN_REAL_SIGNATURE && *s != SENTINEL_SIGNATURE)
}

/// Tool turns match by tool_id at fill time — visible/tool_names are in the request JSON.
/// Only text-only turns keep visible so prefix matching still works after restart.
fn persist_visible<'a>(tool_ids: &[String], visible: &'a str) -> &'a str {
    if tool_ids.is_empty() {
        visible
    } else {
        ""
    }
}

fn pack_thought(s: &str) -> Vec<u8> {
    if s.len() >= MIN_GZIP_THOUGHT {
        let mut enc = GzEncoder::new(Vec::with_capacity(s.len() / 2), Compression::fast());
        if enc.write_all(s.as_bytes()).is_ok() {
            if let Ok(buf) = enc.finish() {
                if buf.len() + THOUGHT_GZIP_MAGIC.len() < s.len() {
                    let mut out = Vec::with_capacity(THOUGHT_GZIP_MAGIC.len() + buf.len());
                    out.extend_from_slice(THOUGHT_GZIP_MAGIC);
                    out.extend_from_slice(&buf);
                    return out;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(THOUGHT_RAW_MAGIC.len() + s.len());
    out.extend_from_slice(THOUGHT_RAW_MAGIC);
    out.extend_from_slice(s.as_bytes());
    out
}

fn unpack_thought(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(THOUGHT_GZIP_MAGIC) {
        let mut decoder = GzDecoder::new(rest);
        let mut s = String::new();
        if decoder.read_to_string(&mut s).is_ok() {
            return s;
        }
    }
    if let Some(rest) = bytes.strip_prefix(THOUGHT_RAW_MAGIC) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn get_proxy_db_path() -> Result<PathBuf, String> {
    let data_dir = crate::modules::account::get_data_dir()?;
    Ok(data_dir.join("proxy_logs.db"))
}

pub fn get_thinking_db_path() -> Result<PathBuf, String> {
    let data_dir = crate::modules::account::get_data_dir()?;
    Ok(data_dir.join("thinking_store.db"))
}

fn apply_fast_pragmas(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| e.to_string())?;
    let _ = conn.pragma_update(None, "cache_size", -64000);
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    let _ = conn.pragma_update(None, "mmap_size", 268435456);
    Ok(())
}

fn connect_db() -> Result<Connection, String> {
    let db_path = get_proxy_db_path()?;
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    apply_fast_pragmas(&conn)?;
    Ok(conn)
}

fn init_thinking_schema(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS thinking_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            thought TEXT NOT NULL,
            signature TEXT,
            tool_ids TEXT NOT NULL,
            tool_names TEXT NOT NULL,
            visible TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_accessed INTEGER
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_session ON thinking_records (session_key, created_at ASC)",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_fp ON thinking_records (session_key, fingerprint)",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_latest ON thinking_records (session_key, id DESC)",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thinking_rec_accessed ON thinking_records (last_accessed ASC)",
        [],
    );
    conn.execute(
        "CREATE TABLE IF NOT EXISTS thinking_sessions (
            session_key TEXT PRIMARY KEY,
            last_accessed INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS thinking_meta (
            k TEXT PRIMARY KEY,
            v TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn open_thinking_db() -> Result<Connection, String> {
    let db_path = get_thinking_db_path()?;
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    apply_fast_pragmas(&conn)?;
    init_thinking_schema(&conn)?;
    Ok(conn)
}

/// Process-lifetime connection to thinking_store.db.
/// Fill/hydrate must not open proxy_logs.db (it can be multi-GB on HDD).
fn thinking_db() -> Result<MutexGuard<'static, Connection>, String> {
    static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
    if DB.get().is_none() {
        let conn = open_thinking_db()?;
        let _ = DB.set(Mutex::new(conn));
    }
    DB.get()
        .ok_or_else(|| "thinking db was not initialized".to_string())?
        .lock()
        .map_err(|e| format!("thinking db lock: {e}"))
}

fn mark_thinking_imported(conn: &Connection) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO thinking_meta (k, v) VALUES ('imported_from_proxy_logs', '1')",
        [],
    );
}

/// Copy old thinking rows out of proxy_logs.db into thinking_store.db.
/// Never deletes the log DB. Old uncompressed rows stay readable via unpack_thought.
fn migrate_thinking_from_logs() -> Result<(), String> {
    let conn = thinking_db()?;
    let imported: Option<String> = conn
        .query_row(
            "SELECT v FROM thinking_meta WHERE k = 'imported_from_proxy_logs'",
            [],
            |r| r.get(0),
        )
        .ok();
    if imported.as_deref() == Some("1") {
        return Ok(());
    }

    let logs_path = get_proxy_db_path()?;
    if !logs_path.exists() {
        mark_thinking_imported(&conn);
        return Ok(());
    }

    let escaped = logs_path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "''");
    if conn
        .execute(&format!("ATTACH DATABASE '{}' AS logs", escaped), [])
        .is_err()
    {
        return Ok(());
    }

    let has_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM logs.sqlite_master WHERE type='table' AND name='thinking_records'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if has_table == 0 {
        let _ = conn.execute("DETACH DATABASE logs", []);
        mark_thinking_imported(&conn);
        return Ok(());
    }

    // Copy only rows not already present. Do not gzip/rewrite on import — that
    // would stall HDD by touching every old thought blob at startup.
    let copy_with_accessed = "INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at, last_accessed)
             SELECT src.session_key, src.fingerprint, src.thought, src.signature, src.tool_ids, src.tool_names, src.visible, src.created_at,
                    COALESCE(src.last_accessed, src.created_at)
             FROM logs.thinking_records src
             WHERE NOT EXISTS (
                SELECT 1 FROM thinking_records t
                WHERE t.session_key = src.session_key
                  AND t.fingerprint = src.fingerprint
                  AND t.created_at = src.created_at
             )";
    let copy_basic = "INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at, last_accessed)
             SELECT src.session_key, src.fingerprint, src.thought, src.signature, src.tool_ids, src.tool_names, src.visible, src.created_at, src.created_at
             FROM logs.thinking_records src
             WHERE NOT EXISTS (
                SELECT 1 FROM thinking_records t
                WHERE t.session_key = src.session_key
                  AND t.fingerprint = src.fingerprint
                  AND t.created_at = src.created_at
             )";
    let copied = match conn.execute(copy_with_accessed, []) {
        Ok(n) => n,
        Err(_) => match conn.execute(copy_basic, []) {
            Ok(n) => n,
            Err(e) => {
                let _ = conn.execute("DETACH DATABASE logs", []);
                tracing::warn!("[ThinkingStore] Import from proxy_logs.db failed (will retry next start): {e}");
                return Ok(());
            }
        },
    };
    let _ = conn.execute(
        "INSERT OR IGNORE INTO thinking_sessions (session_key, last_accessed)
         SELECT session_key, MAX(created_at) FROM thinking_records GROUP BY session_key",
        [],
    );
    let _ = conn.execute("DETACH DATABASE logs", []);
    mark_thinking_imported(&conn);
    if copied > 0 {
        tracing::info!(
            "[ThinkingStore] Imported {} thinking row(s) from proxy_logs.db (old file kept as backup)",
            copied
        );
    }
    Ok(())
}

pub fn init_db() -> Result<(), String> {
    // connect_db will initialize WAL mode and other pragmas
    let conn = connect_db()?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS request_logs (
            id TEXT PRIMARY KEY,
            timestamp INTEGER,
            method TEXT,
            url TEXT,
            status INTEGER,
            duration INTEGER,
            model TEXT,
            error TEXT
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Try to add new columns (ignore errors if they exist)
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN request_body TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN upstream_request_body TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN response_body TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN input_tokens INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN output_tokens INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE request_logs ADD COLUMN cached_tokens INTEGER",
        [],
    );
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN account_email TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN mapped_model TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN protocol TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN client_ip TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN username TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN request_headers TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN upstream_request_headers TEXT", []);
    let _ = conn.execute("ALTER TABLE request_logs ADD COLUMN response_headers TEXT", []);

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_timestamp ON request_logs (timestamp DESC)",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Add status index for faster stats queries
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_status ON request_logs (status)",
        [],
    )
    .map_err(|e| e.to_string())?;

    // 高效复合索引：状态与时间戳倒序（针对错误筛选与分页排序，极大提升大数据量下的响应速度）
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_status_timestamp ON request_logs (status, timestamp DESC)",
        [],
    );

    // 复合索引：模型与时间戳倒序（针对模型级日志过滤与排序）
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_model_timestamp ON request_logs (model, timestamp DESC)",
        [],
    );

    // 复合索引：账号邮箱与时间戳倒序（针对多用户/多账号过滤）
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_account_timestamp ON request_logs (account_email, timestamp DESC)",
        [],
    );

    // 复合索引：客户端IP与时间戳倒序（针对安全审计与IP过滤）
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_client_ip_timestamp ON request_logs (client_ip, timestamp DESC)",
        [],
    );

    // 复合索引：用户名与时间戳倒序
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_username_timestamp ON request_logs (username, timestamp DESC)",
        [],
    );

    // 单列索引：协议类型
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_protocol ON request_logs (protocol)",
        [],
    );

    // 单列索引：请求方法
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_method ON request_logs (method)",
        [],
    );

    // 持久化工具签名表 (支持代理重启后根据 tool_id 秒级恢复真实加密签名)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tool_signatures (
            tool_id TEXT PRIMARY KEY,
            signature TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    let _ = conn.execute("CREATE INDEX IF NOT EXISTS idx_tool_sig_created ON tool_signatures (created_at DESC)", []);

    drop(conn);
    migrate_thinking_from_logs()?;

    Ok(())
}

fn map_request_log_row(row: &rusqlite::Row) -> rusqlite::Result<ProxyRequestLog> {
    Ok(ProxyRequestLog {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        method: row.get(2)?,
        url: row.get(3)?,
        status: row.get(4)?,
        duration: row.get(5)?,
        model: row.get(6)?,
        error: row.get(7)?,
        request_body: row.get(8).unwrap_or(None),
        upstream_request_body: row.get(9).unwrap_or(None),
        response_body: row.get(10).unwrap_or(None),
        input_tokens: row.get(11).unwrap_or(None),
        output_tokens: row.get(12).unwrap_or(None),
        cached_tokens: row.get(13).unwrap_or(None),
        account_email: row.get(14).unwrap_or(None),
        mapped_model: row.get(15).unwrap_or(None),
        protocol: row.get(16).unwrap_or(None),
        client_ip: row.get(17).unwrap_or(None),
        username: row.get(18).unwrap_or(None),
        request_headers: row.get(19).unwrap_or(None),
        upstream_request_headers: row.get(20).unwrap_or(None),
        response_headers: row.get(21).unwrap_or(None),
    })
}

pub fn save_tool_signature(tool_id: &str, signature: &str) -> Result<(), String> {
    if tool_id.is_empty() || signature.is_empty() {
        return Ok(());
    }
    let conn = connect_db()?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT OR REPLACE INTO tool_signatures (tool_id, signature, created_at) VALUES (?1, ?2, ?3)",
        params![tool_id, signature, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_tool_signature(tool_id: &str) -> Result<Option<String>, String> {
    if tool_id.is_empty() {
        return Ok(None);
    }
    let conn = connect_db()?;
    let mut stmt = conn
        .prepare("SELECT signature FROM tool_signatures WHERE tool_id = ?1 LIMIT 1")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![tool_id]).map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let sig: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(sig))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone)]
pub struct PersistedThinkingRecord {
    pub fingerprint: String,
    pub thought: String,
    pub signature: Option<String>,
    pub tool_ids: Vec<String>,
    pub tool_names: Vec<String>,
    pub visible: String,
}

pub fn save_thinking_record(
    session_key: &str,
    fingerprint: &str,
    thought: &str,
    signature: Option<&str>,
    tool_ids: &[String],
    _tool_names: &[String],
    visible: &str,
) -> Result<(), String> {
    if session_key.is_empty() {
        return Ok(());
    }
    let conn = thinking_db()?;
    let now = chrono::Utc::now().timestamp_millis();
    let tool_ids_json = serde_json::to_string(tool_ids).unwrap_or_else(|_| "[]".to_string());
    // tool_names / full visible for tool turns are reconstructable from the next
    // request JSON at fill time. Do not write them.
    let visible_persist = persist_visible(tool_ids, visible);
    let packed_thought = pack_thought(thought);
    let signature = persist_signature(signature);

    // Align with in-memory ThinkingStore: only merge consecutive chunks of the
    // current (latest) turn. Never rewrite an older turn that happens to share
    // a fingerprint (e.g. two "你好" replies in the same session).
    let latest_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM thinking_records WHERE session_key = ?1 ORDER BY id DESC LIMIT 1",
            params![session_key],
            |r| r.get(0),
        )
        .ok();

    let updated = if let Some(id) = latest_id {
        conn.execute(
            "UPDATE thinking_records
             SET thought = ?1, signature = ?2, tool_ids = ?3, tool_names = '[]', visible = ?4, created_at = ?5
             WHERE id = ?6 AND fingerprint = ?7",
            params![
                packed_thought.as_slice(),
                signature,
                &tool_ids_json,
                visible_persist,
                now,
                id,
                fingerprint,
            ],
        )
        .map_err(|e| e.to_string())?
    } else {
        0
    };

    if updated == 0 {
        conn.execute(
            "INSERT INTO thinking_records (session_key, fingerprint, thought, signature, tool_ids, tool_names, visible, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, '[]', ?6, ?7)",
            params![
                session_key,
                fingerprint,
                packed_thought.as_slice(),
                signature,
                &tool_ids_json,
                visible_persist,
                now,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    let _ = conn.execute(
        "INSERT INTO thinking_sessions (session_key, last_accessed) VALUES (?1, ?2)
         ON CONFLICT(session_key) DO UPDATE SET last_accessed = excluded.last_accessed",
        params![session_key, now],
    );
    Ok(())
}

pub fn load_thinking_records(session_key: &str) -> Result<Vec<PersistedThinkingRecord>, String> {
    if session_key.is_empty() {
        return Ok(Vec::new());
    }
    let conn = thinking_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT fingerprint, thought, signature, tool_ids, tool_names, visible 
             FROM thinking_records 
             WHERE session_key = ?1 
             ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![session_key], |row| {
            let fp: String = row.get(0)?;
            let thought_raw: Vec<u8> = row.get(1)?;
            let signature: Option<String> = row.get(2)?;
            let tool_ids_str: String = row.get(3)?;
            let tool_names_str: String = row.get(4)?;
            let visible: String = row.get(5)?;
            Ok((fp, thought_raw, signature, tool_ids_str, tool_names_str, visible))
        })
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for row in rows {
        if let Ok((fp, thought_raw, signature, tool_ids_str, tool_names_str, visible)) = row {
            let tool_ids: Vec<String> = serde_json::from_str(&tool_ids_str).unwrap_or_default();
            let tool_names: Vec<String> = serde_json::from_str(&tool_names_str).unwrap_or_default();
            result.push(PersistedThinkingRecord {
                fingerprint: fp,
                thought: unpack_thought(&thought_raw),
                signature: persist_signature(signature.as_deref()).map(str::to_string),
                tool_ids,
                tool_names,
                visible,
            });
        }
    }
    Ok(result)
}

pub fn touch_thinking_session(session_key: &str) -> Result<usize, String> {
    if session_key.is_empty() {
        return Ok(0);
    }
    let conn = thinking_db()?;
    let now = chrono::Utc::now().timestamp_millis();
    // Touch a 1-row session table. Never UPDATE thinking_records here — that
    // rewrites every thought/visible TEXT blob for the session.
    conn.execute(
        "INSERT INTO thinking_sessions (session_key, last_accessed) VALUES (?1, ?2)
         ON CONFLICT(session_key) DO UPDATE SET last_accessed = excluded.last_accessed",
        params![session_key, now],
    )
    .map_err(|e| e.to_string())
}

pub fn delete_thinking_records_except_fingerprints(
    session_key: &str,
    keep_fps: &[String],
) -> Result<usize, String> {
    if session_key.is_empty() || keep_fps.is_empty() {
        return Ok(0);
    }
    let conn = thinking_db()?;
    let fps_json = serde_json::to_string(keep_fps).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "DELETE FROM thinking_records
         WHERE session_key = ?1
         AND fingerprint NOT IN (SELECT value FROM json_each(?2))",
        params![session_key, fps_json],
    )
    .map_err(|e| e.to_string())
}

pub fn delete_thinking_records_for_session(session_key: &str) -> Result<usize, String> {
    let conn = thinking_db()?;
    let _ = conn.execute(
        "DELETE FROM thinking_sessions WHERE session_key = ?1",
        params![session_key],
    );
    conn.execute(
        "DELETE FROM thinking_records WHERE session_key = ?1",
        params![session_key],
    )
    .map_err(|e| e.to_string())
}

pub fn cleanup_old_thinking_records(days: i64) -> Result<usize, String> {
    let cutoff = chrono::Utc::now().timestamp_millis() - (days * 24 * 3600 * 1000);
    let deleted_tools = connect_db()
        .ok()
        .and_then(|conn| {
            conn.execute(
                "DELETE FROM tool_signatures WHERE created_at < ?1",
                params![cutoff],
            )
            .ok()
        })
        .unwrap_or(0);
    let conn = thinking_db()?;
    let deleted_records = conn
        .execute(
            "DELETE FROM thinking_records WHERE session_key IN (
                SELECT session_key FROM thinking_sessions WHERE last_accessed < ?1
             ) OR (
                session_key NOT IN (SELECT session_key FROM thinking_sessions)
                AND COALESCE(last_accessed, created_at) < ?1
             )",
            params![cutoff],
        )
        .unwrap_or(0);
    let _ = conn.execute(
        "DELETE FROM thinking_sessions WHERE last_accessed < ?1",
        params![cutoff],
    );
    Ok(deleted_tools + deleted_records)
}

pub fn apply_retention(policy: &LogRetentionConfig) -> Result<(usize, usize), String> {
    let conn = connect_db()?;
    apply_retention_with_connection(&conn, policy)
}

fn apply_retention_with_connection(
    conn: &Connection,
    policy: &LogRetentionConfig,
) -> Result<(usize, usize), String> {
    let now = chrono::Utc::now().timestamp_millis();
    let body_cutoff = now - (policy.max_body_age_hours as i64 * 3600 * 1000);
    let age_cutoff = now - (policy.max_age_days as i64 * 24 * 3600 * 1000);
    let bodies_cleared = conn.execute(
        "UPDATE request_logs SET request_body = NULL, upstream_request_body = NULL, response_body = NULL WHERE timestamp < ?1 AND (request_body IS NOT NULL OR upstream_request_body IS NOT NULL OR response_body IS NOT NULL)",
        [body_cutoff],
    ).map_err(|e| e.to_string())?;
    let mut rows_deleted = conn
        .execute(
            "DELETE FROM request_logs WHERE timestamp < ?1",
            [age_cutoff],
        )
        .map_err(|e| e.to_string())?;
    if policy.max_rows > 0 {
        rows_deleted += conn.execute(
            "DELETE FROM request_logs WHERE id NOT IN (SELECT id FROM request_logs ORDER BY timestamp DESC LIMIT ?1)",
            [policy.max_rows],
        ).map_err(|e| e.to_string())?;
    }
    let auto_vacuum: i64 = conn
        .pragma_query_value(None, "auto_vacuum", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if auto_vacuum == 0 {
        let db_size = get_proxy_db_path()
            .ok()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if db_size < 1024 * 1024 * 1024 {
            conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")
                .map_err(|e| e.to_string())?;
        } else {
            tracing::warn!(
                "Skipping auto_vacuum migration for proxy logs database larger than 1 GiB"
            );
        }
    }
    conn.execute_batch("PRAGMA incremental_vacuum;")
        .map_err(|e| e.to_string())?;
    Ok((bodies_cleared, rows_deleted))
}

pub fn save_log(log: &ProxyRequestLog) -> Result<(), String> {
    let conn = connect_db()?;

    conn.execute(
        "INSERT INTO request_logs (id, timestamp, method, url, status, duration, model, error, request_body, upstream_request_body, response_body, input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username, request_headers, upstream_request_headers, response_headers)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
        params![
            log.id,
            log.timestamp,
            log.method,
            log.url,
            log.status,
            log.duration,
            log.model,
            log.error,
            log.request_body,
            log.upstream_request_body,
            log.response_body,
            log.input_tokens,
            log.output_tokens,
            log.cached_tokens,
            log.account_email,
            log.mapped_model,
            log.protocol,
            log.client_ip,
            log.username,
            log.request_headers,
            log.upstream_request_headers,
            log.response_headers,
        ],
    ).map_err(|e| e.to_string())?;

    Ok(())
}

/// Get logs summary (without large request_body and response_body fields) with pagination
pub fn get_logs_summary(limit: usize, offset: usize) -> Result<Vec<ProxyRequestLog>, String> {
    let conn = connect_db()?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, method, url, status, duration, model, error,
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers
         FROM request_logs 
         ORDER BY timestamp DESC 
         LIMIT ?1 OFFSET ?2",
        )
        .map_err(|e| e.to_string())?;

    let logs_iter = stmt
        .query_map([limit, offset], map_request_log_row)
        .map_err(|e| e.to_string())?;

    let mut logs = Vec::new();
    for log in logs_iter {
        logs.push(log.map_err(|e| e.to_string())?);
    }
    Ok(logs)
}

/// Get logs (backward compatible, calls get_logs_summary)
pub fn get_logs(limit: usize) -> Result<Vec<ProxyRequestLog>, String> {
    get_logs_summary(limit, 0)
}

pub fn get_stats() -> Result<crate::proxy::monitor::ProxyStats, String> {
    let conn = connect_db()?;

    // Optimized: Use single query instead of three separate queries
    // Use COALESCE to handle NULL values when table is empty (SUM returns NULL for empty set)
    let (total_requests, success_count, error_count): (u64, u64, u64) = conn
        .query_row(
            "SELECT
            COUNT(*) as total,
            COALESCE(SUM(CASE WHEN status >= 200 AND status < 400 THEN 1 ELSE 0 END), 0) as success,
            COALESCE(SUM(CASE WHEN status < 200 OR status >= 400 THEN 1 ELSE 0 END), 0) as error
         FROM request_logs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;

    Ok(crate::proxy::monitor::ProxyStats {
        total_requests,
        success_count,
        error_count,
    })
}

/// Get single log detail (with request_body and response_body)
pub fn get_log_detail(log_id: &str) -> Result<ProxyRequestLog, String> {
    let conn = connect_db()?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, method, url, status, duration, model, error,
                request_body, upstream_request_body, response_body, input_tokens, output_tokens,
                cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                request_headers, upstream_request_headers, response_headers
         FROM request_logs
         WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;

    stmt.query_row([log_id], map_request_log_row)
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod thinking_pack_tests {
    use super::*;

    #[test]
    fn pack_roundtrip_short_and_long() {
        let short = "hello thought";
        assert_eq!(unpack_thought(&pack_thought(short)), short);
        assert!(pack_thought(short).starts_with(THOUGHT_RAW_MAGIC));

        let long = "word ".repeat(2000);
        let packed = pack_thought(&long);
        assert!(
            packed.starts_with(THOUGHT_GZIP_MAGIC),
            "long thought should gzip"
        );
        assert!(packed.len() < long.len());
        assert_eq!(unpack_thought(&packed), long);
    }

    #[test]
    fn unpack_legacy_utf8() {
        assert_eq!(unpack_thought(b"plain old thought"), "plain old thought");
    }

    #[test]
    fn persist_visible_drops_tool_turns() {
        assert_eq!(persist_visible(&["call_1".to_string()], "I will run the tool"), "");
        assert_eq!(persist_visible(&[], "hello"), "hello");
    }
}

#[cfg(test)]
mod retention_tests {
    use super::apply_retention_with_connection;
    use crate::proxy::config::LogRetentionConfig;
    use rusqlite::Connection;

    #[test]
    fn clears_old_bodies_and_limits_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE request_logs (id TEXT PRIMARY KEY, timestamp INTEGER, request_body TEXT, response_body TEXT)").unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO request_logs VALUES ('retained-with-old-body', ?1, 'request', 'response')",
            [now - 25 * 3600 * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs VALUES ('new-1', ?1, NULL, NULL)",
            [now - 30 * 3600 * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs VALUES ('deleted-1', ?1, NULL, NULL)",
            [now - 35 * 3600 * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs VALUES ('deleted-2', ?1, NULL, NULL)",
            [now - 40 * 3600 * 1000],
        )
        .unwrap();
        let policy = LogRetentionConfig {
            max_body_age_hours: 24,
            max_age_days: 30,
            max_rows: 2,
        };
        let (cleared, deleted) = apply_retention_with_connection(&conn, &policy).unwrap();
        assert_eq!(cleared, 1);
        assert_eq!(deleted, 2);
        let body: Option<String> = conn
            .query_row(
                "SELECT request_body FROM request_logs WHERE id = 'retained-with-old-body'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(body, None);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM request_logs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}

/// Cleanup old logs (keep last N days)
pub fn cleanup_old_logs(days: i64) -> Result<usize, String> {
    let conn = connect_db()?;

    // Note: Request log timestamp is stored in milliseconds (chrono::Utc::now().timestamp_millis())
    let cutoff_timestamp_ms = chrono::Utc::now().timestamp_millis() - (days * 24 * 3600 * 1000);

    let deleted = conn
        .execute(
            "DELETE FROM request_logs WHERE timestamp < ?1",
            [cutoff_timestamp_ms],
        )
        .map_err(|e| e.to_string())?;

    // Only execute VACUUM when substantial rows were deleted to avoid saturating disk I/O on startup
    if deleted >= 500 {
        if let Err(e) = conn.execute("VACUUM", []) {
            tracing::warn!("VACUUM failed after log cleanup: {}", e);
        }
    }

    Ok(deleted)
}

/// Limit maximum log count (keep newest N records)
#[allow(dead_code)]
pub fn limit_max_logs(max_count: usize) -> Result<usize, String> {
    let conn = connect_db()?;

    let deleted = conn
        .execute(
            "DELETE FROM request_logs WHERE id NOT IN (
            SELECT id FROM request_logs ORDER BY timestamp DESC LIMIT ?1
        )",
            [max_count],
        )
        .map_err(|e| e.to_string())?;

    // Only execute VACUUM when substantial rows were deleted
    if deleted >= 500 {
        if let Err(e) = conn.execute("VACUUM", []) {
            tracing::warn!("VACUUM failed after limit_max_logs: {}", e);
        }
    }

    Ok(deleted)
}

pub fn clear_logs() -> Result<(), String> {
    let conn = connect_db()?;
    conn.execute("DELETE FROM request_logs", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Get total count of logs in database
pub fn get_logs_count() -> Result<u64, String> {
    let conn = connect_db()?;

    let count: u64 = conn
        .query_row("SELECT COUNT(*) FROM request_logs", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;

    Ok(count)
}

/// Get count of logs matching search filter
/// filter: search text to match in url, method, model, or status
/// errors_only: if true, only count logs with status < 200 or >= 400
pub fn get_logs_count_filtered(filter: &str, errors_only: bool) -> Result<u64, String> {
    let conn = connect_db()?;

    let filter_pattern = format!("%{}%", filter);

    let sql = if errors_only {
        "SELECT COUNT(*) FROM request_logs WHERE (status < 200 OR status >= 400)"
    } else if filter.is_empty() {
        "SELECT COUNT(*) FROM request_logs"
    } else {
        "SELECT COUNT(*) FROM request_logs WHERE
            (url LIKE ?1 OR method LIKE ?1 OR model LIKE ?1 OR CAST(status AS TEXT) LIKE ?1 OR account_email LIKE ?1)"
    };

    let count: u64 = if filter.is_empty() && !errors_only {
        conn.query_row(sql, [], |row| row.get(0))
    } else if errors_only {
        conn.query_row(sql, [], |row| row.get(0))
    } else {
        conn.query_row(sql, [&filter_pattern], |row| row.get(0))
    }
    .map_err(|e| e.to_string())?;

    Ok(count)
}

/// Get logs with search filter and pagination
/// filter: search text to match in url, method, model, or status
/// errors_only: if true, only return logs with status < 200 or >= 400
pub fn get_logs_filtered(
    filter: &str,
    errors_only: bool,
    limit: usize,
    offset: usize,
) -> Result<Vec<ProxyRequestLog>, String> {
    let conn = connect_db()?;

    let filter_pattern = format!("%{}%", filter);

    let sql = if errors_only {
        "SELECT id, timestamp, method, url, status, duration, model, error,
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers
         FROM request_logs
         WHERE (status < 200 OR status >= 400)
         ORDER BY timestamp DESC
         LIMIT ?1 OFFSET ?2"
    } else if filter.is_empty() {
        "SELECT id, timestamp, method, url, status, duration, model, error,
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers
         FROM request_logs
         ORDER BY timestamp DESC
         LIMIT ?1 OFFSET ?2"
    } else {
        "SELECT id, timestamp, method, url, status, duration, model, error,
                NULL as request_body, NULL as upstream_request_body, NULL as response_body,
                input_tokens, output_tokens, cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                NULL as request_headers, NULL as upstream_request_headers, NULL as response_headers
         FROM request_logs
         WHERE (url LIKE ?3 OR method LIKE ?3 OR model LIKE ?3 OR CAST(status AS TEXT) LIKE ?3 OR account_email LIKE ?3 OR client_ip LIKE ?3)
         ORDER BY timestamp DESC
         LIMIT ?1 OFFSET ?2"
    };

    let logs: Vec<ProxyRequestLog> = if filter.is_empty() && !errors_only {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let logs_iter = stmt
            .query_map([limit, offset], map_request_log_row)
            .map_err(|e| e.to_string())?;
        logs_iter.filter_map(|r| r.ok()).collect()
    } else if errors_only {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let logs_iter = stmt
            .query_map([limit, offset], map_request_log_row)
            .map_err(|e| e.to_string())?;
        logs_iter.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let logs_iter = stmt
            .query_map(rusqlite::params![limit, offset, filter_pattern], map_request_log_row)
            .map_err(|e| e.to_string())?;
        logs_iter.filter_map(|r| r.ok()).collect()
    };

    Ok(logs)
}

/// Get all logs with full details for export
pub fn get_all_logs_for_export() -> Result<Vec<ProxyRequestLog>, String> {
    let conn = connect_db()?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, method, url, status, duration, model, error,
                request_body, upstream_request_body, response_body, input_tokens, output_tokens,
                cached_tokens, account_email, mapped_model, protocol, client_ip, username,
                request_headers, upstream_request_headers, response_headers
         FROM request_logs
         ORDER BY timestamp DESC",
        )
        .map_err(|e| e.to_string())?;

    let logs_iter = stmt
        .query_map([], map_request_log_row)
        .map_err(|e| e.to_string())?;

    let mut logs = Vec::new();
    for log in logs_iter {
        logs.push(log.map_err(|e| e.to_string())?);
    }
    Ok(logs)
}

// ... existing code ...

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IpTokenStats {
    pub client_ip: String,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub request_count: i64,
    pub username: Option<String>,
}

/// Get token usage grouped by IP
pub fn get_token_usage_by_ip(limit: usize, hours: i64) -> Result<Vec<IpTokenStats>, String> {
    let conn = connect_db()?;

    // Fix: Database stores timestamp in milliseconds, but we were calculating 'since' in seconds
    // Convert 'hours' to milliseconds
    let since = chrono::Utc::now().timestamp_millis() - (hours * 3600 * 1000);

    // [FIX] 不再从 request_logs 表获取 username，因为该字段可能为空
    // 先获取 IP 统计数据，然后再单独查询每个 IP 的用户名
    let mut stmt = conn
        .prepare(
            "SELECT
            client_ip,
            COALESCE(SUM(input_tokens), 0) + COALESCE(SUM(output_tokens), 0) as total,
            COALESCE(SUM(input_tokens), 0) as input,
            COALESCE(SUM(output_tokens), 0) as output,
            COUNT(*) as cnt
         FROM request_logs
         WHERE timestamp >= ?1 AND client_ip IS NOT NULL AND client_ip != ''
         GROUP BY client_ip
         ORDER BY total DESC
         LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![since, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut stats = Vec::new();
    for row in rows {
        let (client_ip, total_tokens, input_tokens, output_tokens, request_count) =
            row.map_err(|e| e.to_string())?;

        // 从 user_token_db 获取该 IP 关联的用户名
        // 这比从 request_logs 获取更可靠，因为 token_ip_bindings 表在每次 User Token 使用时都会更新
        let username =
            crate::modules::user_token_db::get_username_for_ip(&client_ip).unwrap_or(None);

        stats.push(IpTokenStats {
            client_ip,
            total_tokens,
            input_tokens,
            output_tokens,
            request_count,
            username,
        });
    }

    Ok(stats)
}
