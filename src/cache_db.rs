use std::path::PathBuf;

use crate::error::{TeleError, TeleResult};

pub fn cache_dir() -> TeleResult<PathBuf> {
    crate::config::app_data_dir_checked().map(|d| d.join("cache"))
}

pub fn cache_path(account: &str) -> TeleResult<PathBuf> {
    cache_dir().map(|d| d.join(format!("{account}.cache.db")))
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    chat_name TEXT NOT NULL DEFAULT '',
    sender_id INTEGER,
    sender_name TEXT NOT NULL DEFAULT '',
    date INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL DEFAULT '',
    media_kind TEXT,
    PRIMARY KEY (chat_id, id)
);
CREATE INDEX IF NOT EXISTS idx_messages_chat ON messages(chat_id);
CREATE INDEX IF NOT EXISTS idx_messages_date ON messages(date DESC);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    text, chat_name, sender_name,
    content='messages', content_rowid='rowid'
);
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, text, chat_name, sender_name)
    VALUES (new.rowid, new.text, new.chat_name, new.sender_name);
END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, text, chat_name, sender_name)
    VALUES ('delete', old.rowid, old.text, old.chat_name, old.sender_name);
END;
CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, text, chat_name, sender_name)
    VALUES ('delete', old.rowid, old.text, old.chat_name, old.sender_name);
    INSERT INTO messages_fts(rowid, text, chat_name, sender_name)
    VALUES (new.rowid, new.text, new.chat_name, new.sender_name);
END;
";

async fn open_db(account: &str) -> TeleResult<libsql::Connection> {
    let dir = cache_dir()?;
    // Directory creation and permission hardening are blocking FS calls; run
    // them off the async worker threads (create_dir_private walks metadata
    // and chmods on unix).
    tokio::task::spawn_blocking(move || -> TeleResult<()> {
        std::fs::create_dir_all(&dir).map_err(|e| {
            TeleError::Other(format!("cannot create cache dir {}: {e}", dir.display()))
        })?;
        crate::fs_util::create_dir_private(&dir).map_err(|e| {
            TeleError::Other(format!("cannot restrict cache dir {}: {e}", dir.display()))
        })?;
        Ok(())
    })
    .await
    .map_err(|e| TeleError::Other(format!("cache dir task failed: {e}")))??;
    let path = cache_path(account)?;
    let seed_path = path.clone();
    tokio::task::spawn_blocking(move || -> TeleResult<()> {
        match std::fs::symlink_metadata(&seed_path) {
            Ok(meta) if meta.file_type().is_symlink() => Err(TeleError::Other(format!(
                "refusing to open symlinked cache db at {}",
                seed_path.display()
            ))),
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                crate::fs_util::create_file_private(&seed_path)
                    .map(|_| ())
                    .map_err(|e| {
                        TeleError::Other(format!(
                            "cannot create cache db {}: {e}",
                            seed_path.display()
                        ))
                    })
            }
            Err(e) => Err(TeleError::Other(format!(
                "cannot stat cache db {}: {e}",
                seed_path.display()
            ))),
        }
    })
    .await
    .map_err(|e| TeleError::Other(format!("cache seed task failed: {e}")))??;
    let db = libsql::Builder::new_local(&path)
        .build()
        .await
        .map_err(|e| TeleError::Other(format!("cannot open cache db: {e}")))?;
    let conn = db
        .connect()
        .map_err(|e| TeleError::Other(format!("cannot connect to cache db: {e}")))?;
    conn.execute_batch(SCHEMA)
        .await
        .map_err(|e| TeleError::Other(format!("cannot init cache schema: {e}")))?;
    conn.execute_batch("PRAGMA recursive_triggers = ON;")
        .await
        .map_err(|e| TeleError::Other(format!("cannot set pragma: {e}")))?;
    let restrict_path = path.clone();
    tokio::task::spawn_blocking(move || {
        crate::fs_util::restrict_file_private(&restrict_path)
            .map_err(|e| TeleError::Other(format!("cannot restrict cache db: {e}")))
    })
    .await
    .map_err(|e| TeleError::Other(format!("cache restrict task failed: {e}")))??;
    Ok(conn)
}

#[derive(Debug, Clone, Default)]
pub struct CachedMessage {
    pub id: i32,
    pub chat_id: i64,
    pub chat_name: String,
    pub sender_id: Option<i64>,
    pub sender_name: String,
    pub date: i64,
    pub text: String,
    pub media_kind: Option<String>,
}

pub async fn store_messages(account: &str, msgs: &[CachedMessage]) -> TeleResult<usize> {
    if msgs.is_empty() {
        return Ok(0);
    }
    let conn = open_db(account).await?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .await
        .map_err(|e| TeleError::Other(format!("cache begin failed: {e}")))?;
    let mut stored = 0usize;
    let mut failure: Option<TeleError> = None;
    for m in msgs {
        match conn
            .execute(
                "INSERT OR REPLACE INTO messages (id, chat_id, chat_name, sender_id, sender_name, date, text, media_kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                libsql::params![
                    m.id as i64,
                    m.chat_id,
                    m.chat_name.clone(),
                    m.sender_id.unwrap_or(0),
                    m.sender_name.clone(),
                    m.date,
                    m.text.clone(),
                    m.media_kind.clone().unwrap_or_default(),
                ],
            )
            .await
        {
            Ok(n) => stored += n as usize,
            Err(e) => {
                failure = Some(TeleError::Other(format!("cache insert failed: {e}")));
                break;
            }
        }
    }
    match failure {
        Some(e) => {
            let _ = conn.execute_batch("ROLLBACK").await;
            Err(e)
        }
        None => {
            conn.execute_batch("COMMIT")
                .await
                .map_err(|e| TeleError::Other(format!("cache commit failed: {e}")))?;
            Ok(stored)
        }
    }
}

pub async fn search_cache(
    account: &str,
    query: &str,
    chat_id: Option<i64>,
    limit: u32,
) -> TeleResult<Vec<CachedMessage>> {
    let conn = open_db(account).await?;
    let mut out = Vec::new();
    if query.trim().is_empty() {
        let sql = if chat_id.is_some() {
            "SELECT id, chat_id, chat_name, sender_id, sender_name, date, text, media_kind FROM messages WHERE chat_id = ?1 ORDER BY date DESC LIMIT ?2"
        } else {
            "SELECT id, chat_id, chat_name, sender_id, sender_name, date, text, media_kind FROM messages ORDER BY date DESC LIMIT ?1"
        };
        let mut rows = if let Some(cid) = chat_id {
            conn.query(sql, libsql::params![cid, limit as i64])
                .await
                .map_err(|e| TeleError::Other(format!("cache query failed: {e}")))?
        } else {
            conn.query(sql, libsql::params![limit as i64])
                .await
                .map_err(|e| TeleError::Other(format!("cache query failed: {e}")))?
        };
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| TeleError::Other(format!("cache row failed: {e}")))?
        {
            out.push(row_to_message(&row)?);
        }
        return Ok(out);
    }
    let fts_query = fts_escape(query);
    let sql = if chat_id.is_some() {
        "SELECT m.id, m.chat_id, m.chat_name, m.sender_id, m.sender_name, m.date, m.text, m.media_kind FROM messages_fts f JOIN messages m ON m.rowid = f.rowid WHERE messages_fts MATCH ?1 AND m.chat_id = ?2 ORDER BY m.date DESC LIMIT ?3"
    } else {
        "SELECT m.id, m.chat_id, m.chat_name, m.sender_id, m.sender_name, m.date, m.text, m.media_kind FROM messages_fts f JOIN messages m ON m.rowid = f.rowid WHERE messages_fts MATCH ?1 ORDER BY m.date DESC LIMIT ?2"
    };
    let mut rows = if let Some(cid) = chat_id {
        conn.query(sql, libsql::params![fts_query, cid, limit as i64])
            .await
            .map_err(|e| TeleError::Other(format!("cache fts query failed: {e}")))?
    } else {
        conn.query(sql, libsql::params![fts_query, limit as i64])
            .await
            .map_err(|e| TeleError::Other(format!("cache fts query failed: {e}")))?
    };
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| TeleError::Other(format!("cache row failed: {e}")))?
    {
        out.push(row_to_message(&row)?);
    }
    Ok(out)
}

// Wrap the query in double quotes so FTS5 treats it as a phrase, not query
// syntax. Doubling embedded quotes keeps user input from closing the phrase
// early or injecting column filters (e.g. `LIVE-TEST` would otherwise parse
// as `TEST` being a column name).
fn fts_escape(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

fn row_to_message(row: &libsql::Row) -> TeleResult<CachedMessage> {
    let get_i64 = |i: i32| -> i64 {
        match row.get::<libsql::Value>(i) {
            Ok(libsql::Value::Integer(v)) => v,
            Ok(libsql::Value::Real(v)) => v as i64,
            _ => 0,
        }
    };
    let get_str = |i: i32| -> String {
        match row.get::<libsql::Value>(i) {
            Ok(libsql::Value::Text(s)) => s,
            Ok(libsql::Value::Blob(b)) => String::from_utf8_lossy(&b).into_owned(),
            _ => String::new(),
        }
    };
    Ok(CachedMessage {
        id: get_i64(0) as i32,
        chat_id: get_i64(1),
        chat_name: get_str(2),
        sender_id: {
            let v = get_i64(3);
            if v == 0 {
                None
            } else {
                Some(v)
            }
        },
        sender_name: get_str(4),
        date: get_i64(5),
        text: get_str(6),
        media_kind: {
            let s = get_str(7);
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        },
    })
}

pub async fn cache_stats(account: &str) -> TeleResult<serde_json::Value> {
    let conn = open_db(account).await?;
    let mut rows = conn
        .query(
            "SELECT COUNT(*), COUNT(DISTINCT chat_id), MIN(date), MAX(date) FROM messages",
            libsql::params![],
        )
        .await
        .map_err(|e| TeleError::Other(format!("cache stats failed: {e}")))?;
    let (total, chats, min_date, max_date) = if let Some(row) = rows
        .next()
        .await
        .map_err(|e| TeleError::Other(format!("cache stats row failed: {e}")))?
    {
        let get = |i: i32| -> i64 {
            match row.get::<libsql::Value>(i) {
                Ok(libsql::Value::Integer(v)) => v,
                Ok(libsql::Value::Real(v)) => v as i64,
                _ => 0,
            }
        };
        (get(0), get(1), get(2), get(3))
    } else {
        (0, 0, 0, 0)
    };
    let path = cache_path(account)?;
    let bytes =
        tokio::task::spawn_blocking(move || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0))
            .await
            .unwrap_or(0);
    Ok(serde_json::json!({
        "account": account,
        "messages": total,
        "chats": chats,
        "oldest_date": min_date,
        "newest_date": max_date,
        "bytes": bytes,
    }))
}

pub async fn clear_cache(account: &str) -> TeleResult<serde_json::Value> {
    let conn = open_db(account).await?;
    let deleted = conn
        .execute("DELETE FROM messages", libsql::params![])
        .await
        .map_err(|e| TeleError::Other(format!("cache clear failed: {e}")))?;
    Ok(serde_json::json!({ "account": account, "deleted": deleted, "cleared": true }))
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    fn test_account(tag: &str) -> String {
        format!("cache-test-{tag}-{}", std::process::id())
    }

    #[test]
    fn fts_escape_wraps_in_quotes() {
        assert_eq!(fts_escape("deploy"), "\"deploy\"");
    }

    #[test]
    fn fts_escape_neutralizes_column_syntax_and_injection() {
        // A hyphen would otherwise parse as `column:term` in FTS5.
        assert_eq!(fts_escape("LIVE-TEST"), "\"LIVE-TEST\"");
        // Embedded quotes are doubled so the phrase cannot be closed early.
        assert_eq!(fts_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    fn with_test_appdir<F, T>(f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let out = f();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[tokio::test]
    async fn store_and_search_roundtrip() {
        with_test_appdir(|| {});
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-rt-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let account = test_account("rt");
        let msgs = vec![
            CachedMessage {
                id: 1,
                chat_id: 100,
                chat_name: "team".to_string(),
                sender_id: Some(5),
                sender_name: "alice".to_string(),
                date: 1700000000,
                text: "deploy complete".to_string(),
                media_kind: None,
            },
            CachedMessage {
                id: 2,
                chat_id: 100,
                chat_name: "team".to_string(),
                sender_id: Some(6),
                sender_name: "bob".to_string(),
                date: 1700000001,
                text: "great work".to_string(),
                media_kind: None,
            },
        ];
        let stored = store_messages(&account, &msgs).await.unwrap();
        assert_eq!(stored, 2);
        let found = search_cache(&account, "deploy", None, 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, 1);
        assert_eq!(found[0].sender_name, "alice");
        let all = search_cache(&account, "", None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, 2);
        let scoped = search_cache(&account, "", Some(100), 10).await.unwrap();
        assert_eq!(scoped.len(), 2);
        let miss = search_cache(&account, "", Some(999), 10).await.unwrap();
        assert!(miss.is_empty());
        // FTS branch WITH a chat filter: quoted query + chat scope takes the
        // three-param SQL arm; chat scoping must actually filter matches.
        let fts_scoped = search_cache(&account, "\"deploy\"", Some(100), 10)
            .await
            .unwrap();
        assert_eq!(fts_scoped.len(), 1, "FTS + chat filter hit");
        assert_eq!(fts_scoped[0].id, 1);
        let fts_other_chat = search_cache(&account, "\"deploy\"", Some(999), 10)
            .await
            .unwrap();
        assert!(fts_other_chat.is_empty(), "FTS + chat filter miss");
        let stats = cache_stats(&account).await.unwrap();
        assert_eq!(stats["messages"], 2);
        assert_eq!(stats["chats"], 1);
        let cleared = clear_cache(&account).await.unwrap();
        assert_eq!(cleared["deleted"], 2);
        let empty = search_cache(&account, "", None, 10).await.unwrap();
        assert!(empty.is_empty());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn replace_keeps_fts_index_in_sync() {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-fts-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let account = test_account("fts");
        let v1 = vec![CachedMessage {
            id: 1,
            chat_id: 100,
            chat_name: "team".to_string(),
            sender_id: Some(5),
            sender_name: "alice".to_string(),
            date: 1700000000,
            text: "originaltext".to_string(),
            media_kind: None,
        }];
        store_messages(&account, &v1).await.unwrap();
        let v2 = vec![CachedMessage {
            id: 1,
            chat_id: 100,
            chat_name: "team".to_string(),
            sender_id: Some(5),
            sender_name: "alice".to_string(),
            date: 1700000001,
            text: "editedtext".to_string(),
            media_kind: None,
        }];
        store_messages(&account, &v2).await.unwrap();
        let stale = search_cache(&account, "originaltext", None, 10)
            .await
            .unwrap();
        assert!(stale.is_empty());
        let fresh = search_cache(&account, "editedtext", None, 10)
            .await
            .unwrap();
        assert_eq!(fresh.len(), 1);
        let conn = open_db(&account).await.unwrap();
        // Probe: is recursive_triggers actually on, and how many index docs exist
        // for the replaced-away term? A stale doc means REPLACE bypassed the
        // external-content FTS delete trigger.
        let mut rows = conn.query("PRAGMA recursive_triggers", ()).await.unwrap();
        let rt = rows
            .next()
            .await
            .unwrap()
            .map(|r| match r.get::<libsql::Value>(0) {
                Ok(libsql::Value::Integer(v)) => v,
                _ => -1,
            });
        conn.execute_batch(
            "DROP TABLE IF EXISTS probe_vocab; CREATE VIRTUAL TABLE probe_vocab USING fts5vocab(messages_fts, 'row');",
        )
        .await
        .unwrap();
        let mut rows = conn
            .query(
                "SELECT count(*) FROM probe_vocab WHERE term = 'originaltext'",
                (),
            )
            .await
            .unwrap();
        let stale_docs = rows
            .next()
            .await
            .unwrap()
            .map(|r| match r.get::<libsql::Value>(0) {
                Ok(libsql::Value::Integer(v)) => v,
                _ => -1,
            });
        conn.execute(
            "INSERT INTO messages_fts(messages_fts) VALUES('integrity-check')",
            (),
        )
        .await
        .expect("fts5 integrity-check failed: stale index docs after replace");
        assert_eq!(
            rt,
            Some(1),
            "PRAGMA recursive_triggers must be ON for this connection"
        );
        assert_eq!(
            stale_docs,
            Some(0),
            "fts index must not keep docs for replaced-away text"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn update_keeps_fts_index_in_sync() {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-ftsu-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let account = test_account("ftsu");
        store_messages(&account, &[atomic_message(1, "updatemeoriginal")])
            .await
            .unwrap();
        let conn = open_db(&account).await.unwrap();
        conn.execute(
            "UPDATE messages SET text = 'updatededited' WHERE id = 1 AND chat_id = 100",
            (),
        )
        .await
        .unwrap();
        let stale = search_cache(&account, "updatemeoriginal", None, 10)
            .await
            .unwrap();
        assert!(stale.is_empty());
        let fresh = search_cache(&account, "updatededited", None, 10)
            .await
            .unwrap();
        assert_eq!(fresh.len(), 1);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn store_empty_is_noop() {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-empty-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let stored = store_messages(&test_account("empty"), &[]).await.unwrap();
        assert_eq!(stored, 0);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn atomic_message(id: i32, text: &str) -> CachedMessage {
        CachedMessage {
            id,
            chat_id: 100,
            chat_name: "team".to_string(),
            sender_id: Some(5),
            sender_name: "alice".to_string(),
            date: 1700000000 + i64::from(id),
            text: text.to_string(),
            media_kind: None,
        }
    }

    #[tokio::test]
    async fn store_messages_batch_is_atomic() {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-atomic-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let account = test_account("atomic");
        store_messages(&account, &[atomic_message(1, "baseline")])
            .await
            .unwrap();
        let conn = open_db(&account).await.unwrap();
        conn.execute_batch("CREATE TRIGGER cache_atomic_probe BEFORE INSERT ON messages WHEN NEW.text = 'CACHE_ATOMIC_BOOM' BEGIN SELECT RAISE(ABORT, 'cache atomicity probe'); END;")
            .await
            .unwrap();
        let batch = vec![
            atomic_message(2, "first of batch"),
            atomic_message(3, "CACHE_ATOMIC_BOOM"),
            atomic_message(4, "last of batch"),
        ];
        assert!(store_messages(&account, &batch).await.is_err());
        let found = search_cache(&account, "", None, 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "baseline");
        conn.execute_batch("DROP TRIGGER IF EXISTS cache_atomic_probe;")
            .await
            .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_db_refuses_symlinked_file() {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-symlink-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let account = test_account("symlink");
        let path = cache_path(&account).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let target = dir.join("real-cache.db");
        std::fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(open_db(&account).await.is_err());
        let _ = std::fs::remove_file(&path);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cache_db_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "telecli-cache-mode-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let account = test_account("mode");
        store_messages(&account, &[atomic_message(1, "hello")])
            .await
            .unwrap();
        let path = cache_path(&account).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
