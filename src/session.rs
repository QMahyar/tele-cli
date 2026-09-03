use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, SocketAddrV6};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use grammers_client::session::storages::SqliteSession;
use grammers_client::session::types::{DcOption, PeerInfo};
use grammers_client::session::Session as _;

pub const SESSION_FILE_WARNING: &str =
    "session files grant full access to their Telegram account; treat exports as secrets";

const MAX_SESSION_FILE_BYTES: u64 = 64 * 1024 * 1024;

pub fn session_dir() -> PathBuf {
    crate::config::app_data_dir().join("sessions")
}

const WINDOWS_RESERVED_DEVICE_NAMES: [&str; 22] = [
    "con", "nul", "aux", "prn", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

pub fn validate_name(name: &str) -> Result<(), String> {
    if matches!(name, "all" | "." | "..") {
        return Err(format!("invalid account name {name:?}: reserved"));
    }
    if WINDOWS_RESERVED_DEVICE_NAMES
        .iter()
        .any(|device| device.eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "invalid account name {name:?}: reserved Windows device name"
        ));
    }
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid account name {name:?}: use [A-Za-z0-9._-] only"
        ))
    }
}

pub fn session_path(name: &str) -> PathBuf {
    session_dir().join(format!("{name}.session"))
}

pub fn lock_path(name: &str) -> PathBuf {
    session_dir().join(format!("{name}.session.lock"))
}

pub fn list_session_names() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(session_dir()) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(base) = name.strip_suffix(".session") {
            names.push(base.to_string());
        }
    }
    names.sort();
    names
}

const SESSION_SIDECAR_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];

fn sidecar_path(name: &str, suffix: &str) -> PathBuf {
    session_dir().join(format!("{name}.session{suffix}"))
}

fn pre_restrict_sidecars(name: &str) -> anyhow::Result<()> {
    for suffix in SESSION_SIDECAR_SUFFIXES {
        let path = sidecar_path(name, suffix);
        if !path.try_exists()? {
            create_private_new(&path)?;
        }
        crate::fs_util::restrict_file_private(&path)?;
    }
    Ok(())
}

fn create_private_new(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
    }
    #[cfg(windows)]
    {
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
        crate::fs_util::restrict_file_private(path)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
    }
    Ok(())
}

pub(crate) fn sweep_tighten_session_files() {
    let Ok(entries) = std::fs::read_dir(session_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let _ = crate::fs_util::restrict_file_private(&entry.path());
        }
    }
}

fn restrict_session_files(name: &str) -> anyhow::Result<()> {
    let prefix = format!("{name}.session");
    for entry in std::fs::read_dir(session_dir())? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            crate::fs_util::restrict_file_private(&entry.path())?;
        }
    }
    Ok(())
}

fn sweep_session_artifacts(name: &str) -> anyhow::Result<()> {
    let mut targets = vec![session_path(name), lock_path(name)];
    for suffix in SESSION_SIDECAR_SUFFIXES {
        targets.push(sidecar_path(name, suffix));
    }
    let tmp_prefix = format!("{name}.session.tmp-");
    if let Ok(entries) = std::fs::read_dir(session_dir()) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&tmp_prefix) {
                targets.push(entry.path());
            }
        }
    }
    targets
        .iter()
        .try_for_each(|path| match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        })?;
    Ok(())
}

pub async fn remove_session(name: &str) -> anyhow::Result<()> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    if session_path(name).try_exists()? {
        let lock = acquire_lock_file(name).await?;
        let result = sweep_session_artifacts(name);
        drop(lock);
        result?;
    } else {
        sweep_session_artifacts(name)?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct SessionLock {
    file: Option<Arc<std::fs::File>>,
}

impl SessionLock {
    pub(crate) fn new(file: std::fs::File) -> Self {
        Self {
            file: Some(Arc::new(file)),
        }
    }

    pub(crate) fn share(&self) -> Self {
        Self {
            file: self.file.clone(),
        }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        self.file.take();
    }
}

async fn acquire_lock_file(name: &str) -> anyhow::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let lock = opts.open(lock_path(name))?;
    // On Linux, flock() release is asynchronous after File::drop. Retry a few
    // times with a short delay to handle the kernel delay.
    let mut attempt = 0;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) if attempt < 2 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(anyhow::anyhow!(
                    "session {name} is in use by another process"
                ));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub struct LockedSession {
    pub session: SqliteSession,
    pub(crate) lock: SessionLock,
}

pub async fn open_session(name: &str) -> anyhow::Result<LockedSession> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    let path = session_path(name);
    let dir = session_dir();
    crate::config::ensure_app_data_dir()?;
    crate::fs_util::create_dir_private(&dir)?;
    let lock = acquire_lock_file(name).await?;
    pre_restrict_sidecars(name)?;
    if !path.try_exists()? {
        create_private_new(&path)?;
    }
    let session = SqliteSession::open(&path).await?;
    restrict_session_files(name)?;
    Ok(LockedSession {
        session,
        lock: SessionLock::new(lock),
    })
}

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\x00";

#[derive(Debug, Clone)]
pub struct ExportedSession {
    pub account: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct ImportedSession {
    pub account: String,
    pub path: PathBuf,
    pub bytes: u64,
}

pub async fn export_session(name: &str, out: Option<&Path>) -> anyhow::Result<ExportedSession> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    let source = session_path(name);
    if !source.try_exists()? {
        return Err(anyhow::anyhow!(
            "no session file for account {name}; run tele account list to inspect accounts"
        ));
    }
    let _live_lock = acquire_lock_file(name).await?;
    let probe = SqliteSession::open(&source).await?;
    drop(probe);
    let dest = match out {
        Some(path) => path.to_path_buf(),
        None => {
            crate::config::ensure_app_data_dir()?;
            crate::fs_util::create_dir_private(&session_dir())?;
            session_dir().join(format!("{name}.session.export"))
        }
    };
    if let Ok(meta) = std::fs::symlink_metadata(&dest) {
        if meta.file_type().is_symlink() {
            return Err(anyhow::anyhow!(
                "export destination {} is a symlink; refusing to follow it",
                dest.display()
            ));
        }
    }
    if fs_same_file(&dest, &source) {
        return Err(anyhow::anyhow!(
            "destination equals the live session file for {name}"
        ));
    }
    let dest_guard = crate::fs_util::resolve_for_guard(&dest);
    let sessions_guard = crate::fs_util::resolve_for_guard(&session_dir());
    if crate::fs_util::path_under_guard(&dest_guard, &sessions_guard) {
        let allowed = dest
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".session.export"));
        if !allowed {
            return Err(anyhow::anyhow!(
                "refusing to export into {}: destinations inside the sessions directory must end in .session.export to avoid clobbering live sessions",
                session_dir().display()
            ));
        }
    }
    let mut f = crate::fs_util::create_file_private(&dest)
        .map_err(|e| anyhow::anyhow!("failed to create export file {}: {e}", dest.display()))?;
    let mut src = std::fs::File::open(&source)?;
    let size = std::io::copy(&mut src, &mut f).map_err(|e| {
        let _ = std::fs::remove_file(&dest);
        anyhow::anyhow!("failed to copy session to {}: {e}", dest.display())
    })?;
    f.sync_all().ok();
    crate::fs_util::restrict_file_private(&dest)
        .map_err(|e| anyhow::anyhow!("failed to restrict export file {}: {e}", dest.display()))?;
    let sha = {
        let bytes = std::fs::read(&dest)?;
        sha256_hex(&bytes)
    };
    Ok(ExportedSession {
        account: name.to_string(),
        path: dest.clone(),
        bytes: size,
        sha256: sha,
    })
}

fn fs_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

pub fn resolve_import_name(as_name: Option<&str>, source: &Path) -> anyhow::Result<String> {
    let derived;
    let name = match as_name {
        Some(explicit) => explicit,
        None => {
            derived = source
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "cannot derive an account name from {}; pass --as NAME",
                        source.display()
                    )
                })?
                .to_string();
            derived.as_str()
        }
    };
    validate_name(name).map_err(anyhow::Error::msg)?;
    Ok(name.to_string())
}

fn ensure_no_clobber(name: &str, force: bool) -> anyhow::Result<()> {
    if !force && session_path(name).try_exists()? {
        return Err(anyhow::anyhow!(
            "account {name} already exists; pass --force to overwrite"
        ));
    }
    Ok(())
}

fn tmp_session_path(name: &str) -> PathBuf {
    let mut p = session_path(name).into_os_string();
    p.push(format!(".tmp-{}", std::process::id()));
    PathBuf::from(p)
}

fn tmp_sidecar_path(name: &str, suffix: &str) -> PathBuf {
    let mut p = tmp_session_path(name).into_os_string();
    p.push(suffix);
    PathBuf::from(p)
}

fn cleanup_partial_import(name: &str) {
    let _ = std::fs::remove_file(tmp_session_path(name));
    for suffix in SESSION_SIDECAR_SUFFIXES {
        let _ = std::fs::remove_file(tmp_sidecar_path(name, suffix));
    }
}

pub async fn import_session(
    file: &Path,
    as_name: Option<&str>,
    force: bool,
) -> anyhow::Result<ImportedSession> {
    let name = resolve_import_name(as_name, file)?;
    ensure_no_clobber(&name, force)?;
    if !file.is_file() {
        return Err(anyhow::anyhow!(
            "session source is not a readable file: {}",
            file.display()
        ));
    }
    let mut header = [0u8; 16];
    {
        use std::io::Read;
        let mut handle = std::fs::File::open(file)?;
        let mut filled = 0;
        while filled < header.len() {
            match handle.read(&mut header[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        if filled < header.len() || header != *SQLITE_MAGIC {
            return Err(anyhow::anyhow!(
                "not a valid session file: missing SQLite header"
            ));
        }
    }
    crate::config::ensure_app_data_dir()?;
    crate::fs_util::create_dir_private(&session_dir())?;
    let _lock = acquire_lock_file(&name).await?;
    let byte_len = std::fs::metadata(file)?.len();
    if byte_len > MAX_SESSION_FILE_BYTES {
        return Err(anyhow::anyhow!(
            "refusing to import {}: file is {byte_len} bytes, over the {MAX_SESSION_FILE_BYTES}-byte session cap",
            file.display()
        ));
    }
    install_copied_session(&name, file, byte_len).await
}

async fn install_copied_session(
    name: &str,
    source: &Path,
    byte_len: u64,
) -> anyhow::Result<ImportedSession> {
    let path = session_path(name);
    pre_restrict_sidecars(name)?;
    let tmp_path = tmp_session_path(name);
    let _ = std::fs::remove_file(&tmp_path);
    {
        use std::io::{Read, Write};
        let mut src = std::fs::File::open(source)?;
        let mut dst = crate::fs_util::create_file_private(&tmp_path)?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => dst.write_all(&buf[..n])?,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        dst.sync_all()?;
    }
    let probe_result = async {
        let probe = SqliteSession::open(&tmp_path).await?;
        drop(probe);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(e) = probe_result {
        cleanup_partial_import(name);
        return Err(e.context("not a valid session file"));
    }
    std::fs::rename(&tmp_path, &path)?;
    restrict_session_files(name)?;
    Ok(ImportedSession {
        account: name.to_string(),
        path,
        bytes: byte_len,
    })
}

#[derive(Clone)]
pub struct TelethonSessionData {
    pub schema_version: i64,
    pub dc_id: i32,
    pub server_address: String,
    pub port: i32,
    pub auth_key: [u8; 256],
    pub user_id: Option<i64>,
}

const MAX_TELETHON_SCHEMA_VERSION: i64 = 7;

impl std::fmt::Debug for TelethonSessionData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelethonSessionData")
            .field("schema_version", &self.schema_version)
            .field("dc_id", &self.dc_id)
            .field("server_address", &self.server_address)
            .field("port", &self.port)
            .field("auth_key", &"[REDACTED 256 bytes]")
            .field("user_id", &self.user_id)
            .finish()
    }
}

const TELETHON_SESSION_COLUMNS_REQUIRED: [&str; 4] =
    ["dc_id", "server_address", "port", "auth_key"];
const TELETHON_AUTH_KEY_LEN: usize = 256;

fn libsql_value_kind(value: &libsql::Value) -> &'static str {
    match value {
        libsql::Value::Null => "NULL",
        libsql::Value::Integer(_) => "integer",
        libsql::Value::Real(_) => "real",
        libsql::Value::Text(_) => "text",
        libsql::Value::Blob(_) => "blob",
    }
}

fn libsql_value_string(value: &libsql::Value) -> Option<String> {
    match value {
        libsql::Value::Text(s) => Some(s.clone()),
        libsql::Value::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

async fn libsql_table_columns(
    conn: &libsql::Connection,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(anyhow::anyhow!(
            "unsupported table name for PRAGMA: {table}"
        ));
    }
    let mut columns = Vec::new();
    let mut rows = conn
        .query(&format!("PRAGMA table_info('{table}')"), ())
        .await?;
    while let Some(row) = rows.next().await? {
        if let Some(name) = libsql_value_string(&row.get::<libsql::Value>(1)?) {
            columns.push(name);
        }
    }
    Ok(columns)
}

async fn libsql_read_only_conn(path: &Path) -> anyhow::Result<libsql::Connection> {
    let db = libsql::Builder::new_local(path)
        .flags(libsql::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .build()
        .await
        .with_context(|| "cannot open Telethon session file (path logged at debug level)")?;
    crate::output::log_line(
        "debug",
        &format!(
            "telethon session open attempted: {}",
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
        ),
    );
    Ok(db.connect()?)
}

async fn libsql_collect_rows(
    conn: &libsql::Connection,
    sql: &str,
) -> anyhow::Result<Vec<Vec<libsql::Value>>> {
    let mut collected = Vec::new();
    let mut rows = conn.query(sql, ()).await?;
    while let Some(row) = rows.next().await? {
        let width = rows.column_count();
        let mut cells = Vec::with_capacity(width.max(0) as usize);
        for idx in 0..width {
            cells.push(row.get::<libsql::Value>(idx)?);
        }
        collected.push(cells);
    }
    Ok(collected)
}

fn telethon_int_cell(column: &str, value: &libsql::Value) -> anyhow::Result<i64> {
    match value {
        libsql::Value::Integer(i) => Ok(*i),
        other => Err(anyhow::anyhow!(
            "Telethon column {column} should be integer, found {}",
            libsql_value_kind(other)
        )),
    }
}

fn telethon_auth_key_cell(value: &libsql::Value) -> anyhow::Result<[u8; TELETHON_AUTH_KEY_LEN]> {
    match value {
        libsql::Value::Blob(bytes) => bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "Telethon auth_key has {} bytes; expected {TELETHON_AUTH_KEY_LEN}",
                bytes.len()
            )
        }),
        libsql::Value::Null => Err(anyhow::anyhow!("Telethon auth_key is NULL")),
        other => Err(anyhow::anyhow!(
            "Telethon auth_key should be a 256-byte BLOB, found {}",
            libsql_value_kind(other)
        )),
    }
}

pub async fn parse_telethon_session(source: &Path) -> anyhow::Result<TelethonSessionData> {
    if !source.is_file() {
        anyhow::bail!(
            "Telethon session file not found or not a regular file: {}",
            source.display()
        );
    }
    let mut header = [0u8; 16];
    {
        use std::io::Read as _;
        let mut handle = std::fs::File::open(source)?;
        let mut filled = 0;
        while filled < header.len() {
            let n = handle.read(&mut header[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
    }
    if !header.starts_with(SQLITE_MAGIC) {
        anyhow::bail!(
            "not a Telethon session: {} lacks the SQLite file header",
            source.display()
        );
    }
    let conn = libsql_read_only_conn(source).await?;
    let tables: Vec<String> = {
        let rows = libsql_collect_rows(
            &conn,
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        )
        .await?;
        rows.into_iter()
            .filter_map(|cells| cells.first().and_then(libsql_value_string))
            .collect()
    };
    if !tables.iter().any(|t| t == "sessions") {
        anyhow::bail!(
            "not a Telethon session: found tables [{}], expected one named 'sessions'",
            tables.join(", ")
        );
    }
    let columns = libsql_table_columns(&conn, "sessions").await?;
    let missing: Vec<&str> = TELETHON_SESSION_COLUMNS_REQUIRED
        .iter()
        .copied()
        .filter(|required| !columns.iter().any(|found| found == required))
        .collect();
    if !missing.is_empty() {
        anyhow::bail!(
            "not a Telethon session: 'sessions' table lacks required columns [{}]; found columns [{}]",
            missing.join(", "),
            columns.join(", ")
        );
    }
    let schema_version = match tables.iter().position(|t| t == "version") {
        None => 0,
        Some(_) => {
            let version_columns = libsql_table_columns(&conn, "version").await?;
            match ["number", "version"]
                .iter()
                .find(|candidate| version_columns.iter().any(|found| found == *candidate))
            {
                None => 0,
                Some(candidate) => {
                    let sql = format!(
                        "SELECT {candidate} FROM version WHERE {candidate} IS NOT NULL ORDER BY {candidate} DESC LIMIT 1"
                    );
                    match libsql_collect_rows(&conn, &sql)
                        .await?
                        .into_iter()
                        .next()
                        .and_then(|cells| cells.into_iter().next())
                    {
                        Some(libsql::Value::Integer(number)) => number,
                        _ => 0,
                    }
                }
            }
        }
    };
    if schema_version > MAX_TELETHON_SCHEMA_VERSION {
        anyhow::bail!(
            "Telethon session schema v{schema_version} is newer than the highest tested version \
             (v{MAX_TELETHON_SCHEMA_VERSION}); conversion is refused because column layouts may differ"
        );
    }
    let has_user_id = columns.iter().any(|found| found == "user_id");
    let select_sql = if has_user_id {
        "SELECT dc_id, server_address, port, auth_key, user_id FROM sessions ORDER BY dc_id"
    } else {
        "SELECT dc_id, server_address, port, auth_key FROM sessions ORDER BY dc_id"
    };
    let data_rows = libsql_collect_rows(&conn, select_sql).await?;
    match data_rows.len() {
        0 => anyhow::bail!(
            "not a usable Telethon session: 'sessions' table holds no rows, expected exactly one"
        ),
        n @ 2.. => anyhow::bail!(
            "ambiguous Telethon session: 'sessions' table holds {n} rows, expected exactly one"
        ),
        1 => {}
    }
    let cells = &data_rows[0];
    let dc_id = i32::try_from(telethon_int_cell("dc_id", &cells[0])?)
        .map_err(|_| anyhow::anyhow!("Telethon dc_id out of range"))?;
    let server_address = match &cells[1] {
        libsql::Value::Null => {
            anyhow::bail!("Telethon server_address is NULL")
        }
        value @ (libsql::Value::Text(_) | libsql::Value::Blob(_)) => {
            libsql_value_string(value).unwrap_or_default()
        }
        other => {
            anyhow::bail!(
                "Telethon server_address should be text, found {}",
                libsql_value_kind(other)
            )
        }
    };
    let port = i32::try_from(telethon_int_cell("port", &cells[2])?)
        .map_err(|_| anyhow::anyhow!("Telethon port out of range"))?;
    let auth_key = telethon_auth_key_cell(&cells[3])?;
    let user_id = if has_user_id {
        match &cells[4] {
            libsql::Value::Null => None,
            value => Some(telethon_int_cell("user_id", value)?),
        }
    } else {
        None
    };
    Ok(TelethonSessionData {
        schema_version,
        dc_id,
        server_address,
        port,
        auth_key,
        user_id,
    })
}

fn telethon_dc_sockets(
    server_address: &str,
    port: i32,
) -> anyhow::Result<(SocketAddrV4, SocketAddrV6)> {
    if !(1..=65535).contains(&port) {
        return Err(anyhow::anyhow!("invalid Telethon port {port}"));
    }
    let ip: IpAddr = server_address.parse().map_err(|_| {
        anyhow::anyhow!("unsupported Telethon server_address {server_address:?}: not an IP literal")
    })?;
    let port = port as u16;
    Ok(match ip {
        IpAddr::V4(v4) => (
            SocketAddrV4::new(v4, port),
            SocketAddrV6::new(v4.to_ipv6_mapped(), port, 0, 0),
        ),
        IpAddr::V6(v6) => (
            SocketAddrV4::new(v6.to_ipv4().unwrap_or(Ipv4Addr::UNSPECIFIED), port),
            SocketAddrV6::new(v6, port, 0, 0),
        ),
    })
}

pub async fn write_native_from_telethon(
    name: &str,
    data: &TelethonSessionData,
    force: bool,
) -> anyhow::Result<PathBuf> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    ensure_no_clobber(name, force)?;
    let (ipv4, ipv6) = telethon_dc_sockets(&data.server_address, data.port)?;
    crate::config::ensure_app_data_dir()?;
    crate::fs_util::create_dir_private(&session_dir())?;
    let path = session_path(name);
    let lock = acquire_lock_file(name).await?;
    pre_restrict_sidecars(name)?;
    let tmp_path = tmp_session_path(name);
    let _ = std::fs::remove_file(&tmp_path);
    crate::fs_util::write_file_private(&tmp_path, &[])?;
    crate::fs_util::restrict_file_private(&tmp_path)?;
    let session = SqliteSession::open(&tmp_path).await?;
    let write_result = async {
        session.set_home_dc_id(data.dc_id).await?;
        session
            .set_dc_option(&DcOption {
                id: data.dc_id,
                ipv4,
                ipv6,
                auth_key: Some(data.auth_key),
            })
            .await?;
        if let Some(user_id) = data.user_id {
            session
                .cache_peer(&PeerInfo::User {
                    id: user_id,
                    auth: None,
                    bot: Some(false),
                    is_self: Some(true),
                })
                .await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    drop(session);
    if let Err(e) = write_result {
        drop(lock);
        cleanup_partial_import(name);
        return Err(e.context(format!(
            "failed to author native session from Telethon schema v{}",
            data.schema_version
        )));
    }
    std::fs::rename(&tmp_path, &path)?;
    restrict_session_files(name)?;
    Ok(path)
}

pub async fn convert_telethon_session(
    source: &Path,
    as_name: Option<&str>,
    force: bool,
) -> anyhow::Result<ImportedSession> {
    let data = parse_telethon_session(source).await?;
    let name = resolve_import_name(as_name, source)?;
    ensure_no_clobber(&name, force)?;
    let path = write_native_from_telethon(&name, &data, true).await?;
    let bytes = std::fs::metadata(&path)?.len();
    Ok(ImportedSession {
        account: name,
        path,
        bytes,
    })
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use grammers_session::Session;

    #[test]
    fn validate_name_accepts_plain_names() {
        for name in ["work", "home2", "a.b-c_d", "all-1", "All"] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn validate_name_rejects_reserved_names() {
        for name in ["all", ".", ".."] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn validate_name_rejects_empty_and_separators() {
        for name in ["", " ", "a/b", "a\\b", "../x"] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }

    fn test_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("telecli-{tag}-{}", std::process::id()))
    }

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn seed_test_env(dir: &std::path::Path) {
        std::env::set_var("TELE_APP_DIR", dir);
        std::fs::write(
            dir.join("config.toml"),
            "[accounts.me]\n\
             [accounts.work]\n\
             [accounts.ghost]\n\
             [accounts.origin]\n\
             [accounts.restored]\n\
             [accounts.team_a]\n\
             [accounts.busy]\n\
             [accounts.fromtg]\n\
             [accounts.ipv6case]\n\
             [accounts.keyed]\n\
             [accounts.victim]\n\
             [accounts.x]\n\
             ",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn open_session_lock_is_released_on_drop() {
        let _guard = lock_env();
        let dir = test_dir("session-lock-drop");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let first = open_session("work").await.unwrap();
        drop(first);
        let second = open_session("work").await.unwrap();
        drop(second);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_session_keeps_stale_lock_file_on_drop() {
        let _guard = lock_env();
        let dir = test_dir("session-lock-file-stale");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let held = open_session("work").await.unwrap();
        assert!(lock_path("work").exists());
        drop(held);
        assert!(
            lock_path("work").exists(),
            "stale lock file stays behind; the OS lock is what guards exclusivity"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_session_restricts_sqlite_sidecars() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = lock_env();
        let dir = test_dir("session-sidecars");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            drop(held);
            for suffix in ["-journal", "-wal", "-shm"] {
                let path = sidecar_path("work", suffix);
                if !path.exists() {
                    std::fs::write(&path, b"x").unwrap();
                }
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                assert!(
                    mode & 0o600 == 0o600,
                    "sidecar {suffix}: expected read+write for owner, got {mode:04o}"
                );
            }
        }
        remove_session("work").await.unwrap();
        assert!(!sidecar_path("work", "-journal").exists());
        assert!(!sidecar_path("work", "-wal").exists());
        assert!(!sidecar_path("work", "-shm").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_session_rejects_concurrent_use() {
        let _guard = lock_env();
        let dir = test_dir("session-lock-held");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let held = open_session("work").await.unwrap();
        let err = match open_session("work").await {
            Err(e) => e,
            Ok(_) => panic!("second open should fail while lock is held"),
        };
        assert!(err.to_string().contains("is in use by another process"));
        drop(held);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_cleans_session_and_lock_files() {
        let _guard = lock_env();
        let dir = test_dir("session-lock-remove");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let held = open_session("work").await.unwrap();
        drop(held);
        remove_session("work").await.unwrap();
        assert!(!session_path("work").exists());
        assert!(!lock_path("work").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_rejects_in_use_session() {
        let _guard = lock_env();
        let dir = test_dir("session-lock-remove-held");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let held = open_session("work").await.unwrap();
        let err = remove_session("work").await.unwrap_err();
        assert!(
            err.to_string().contains("is in use by another process"),
            "{err}"
        );
        drop(held);
        remove_session("work").await.unwrap();
        assert!(!session_path("work").exists());
        assert!(!lock_path("work").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_tolerates_never_created_session_file() {
        let _guard = lock_env();
        let dir = test_dir("session-lock-remove-never-created");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        remove_session("work").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_matches_fips_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(sha256_hex(&[0u8; 1_000_000]).len(), 64);
    }

    #[tokio::test]
    async fn export_session_copies_bytes_and_reports_sha() {
        let _guard = lock_env();
        let dir = test_dir("export-basic");
        let out = dir.join("out");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&out).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            drop(held);
        }
        let dest = out.join("work.session");
        let exported = export_session("work", Some(&dest)).await.unwrap();
        assert_eq!(exported.account, "work");
        assert_eq!(exported.path, dest);
        let source_bytes = std::fs::read(session_path("work")).unwrap();
        assert_eq!(exported.bytes, source_bytes.len() as u64);
        assert_eq!(
            exported.sha256,
            sha256_hex(&source_bytes),
            "reported digest must match source content"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            source_bytes,
            "copy must be byte-identical"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "export must be owner-only");
        }
        remove_session("work").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn export_refuses_missing_source() {
        let _guard = lock_env();
        let dir = test_dir("export-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let err = export_session("ghost", None).await.unwrap_err();
        assert!(err.to_string().contains("no session file"), "{err}");
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn export_refuses_live_locked_source() {
        let _guard = lock_env();
        let dir = test_dir("export-locked");
        let out = dir.join("out");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&out).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            let err = export_session("work", Some(&out.join("w.session")))
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("in use by another process"),
                "{err}"
            );
            assert!(!out.join("w.session").exists(), "nothing may be written");
            drop(held);
        }
        export_session("work", Some(&out.join("w.session")))
            .await
            .unwrap();
        remove_session("work").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn sqlite_scaffold(path: &Path) {
        let s = SqliteSession::open(path).await.unwrap();
        drop(s);
    }

    #[tokio::test]
    async fn import_export_roundtrip_is_byte_identical() {
        let _guard = lock_env();
        let dir = test_dir("import-roundtrip");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("origin").await.unwrap();
            drop(held);
        }
        let backup = inbox.join("origin.session");
        let first = export_session("origin", Some(&backup)).await.unwrap();
        let imported = import_session(&backup, Some("restored"), false)
            .await
            .unwrap();
        assert_eq!(imported.account, "restored");
        assert_eq!(imported.bytes, first.bytes);
        let reexport_path = dir.join("reexport.session");
        let reexport = export_session("restored", Some(&reexport_path))
            .await
            .unwrap();
        assert_eq!(reexport.sha256, first.sha256, "roundtrip identity");
        assert_eq!(
            std::fs::read(&reexport_path).unwrap(),
            std::fs::read(&backup).unwrap()
        );

        let err = import_session(&backup, Some("restored"), false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--force to overwrite"), "{err}");
        import_session(&backup, Some("restored"), true)
            .await
            .unwrap();

        remove_session("restored").await.unwrap();
        remove_session("origin").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_derives_and_validates_name_from_stem() {
        let _guard = lock_env();
        let dir = test_dir("import-stem");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        seed_test_env(&dir);

        let good = inbox.join("team_a.session");
        sqlite_scaffold(&good).await;
        let imported = import_session(&good, None, false).await.unwrap();
        assert_eq!(imported.account, "team_a");
        assert!(session_path("team_a").exists());

        let bad = inbox.join("bad name.session");
        std::fs::write(&bad, b"x").unwrap();
        let err = import_session(&bad, None, false).await.unwrap_err();
        assert!(err.to_string().contains("invalid account name"), "{err}");
        assert!(
            !session_path("bad name").exists(),
            "invalid names must write nothing"
        );

        let no_stem = inbox.join(".hidden");
        sqlite_scaffold(&no_stem).await;
        let err = import_session(&no_stem, Some("../escape"), false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid account name"), "{err}");

        remove_session("team_a").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_rejects_non_sqlite_garbage() {
        let _guard = lock_env();
        let dir = test_dir("import-garbage");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        seed_test_env(&dir);
        for (tag, body) in [("empty", &b""[..]), ("text", &b"definitely not sqlite"[..])] {
            let junk = inbox.join(format!("{tag}.session"));
            std::fs::write(&junk, body).unwrap();
            let err = import_session(&junk, Some("victim"), false)
                .await
                .unwrap_err();
            assert!(
                err.to_string()
                    .to_lowercase()
                    .contains("not a valid session"),
                "{tag}: {err}"
            );
            assert!(
                !session_path("victim").exists(),
                "rejected imports must clean up"
            );
        }
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_name_rejects_windows_device_names() {
        for name in [
            "con", "CON", "Con", "nul", "NUL", "aux", "AUX", "prn", "PRN", "com1", "COM9", "Com5",
            "lpt1", "LPT9", "Lpt3",
        ] {
            assert!(validate_name(name).is_err(), "{name} must be rejected");
        }
        for name in [
            "console",
            "nullsafe",
            "computer",
            "auxiliary",
            "com0",
            "lpt0",
            "lpt10",
        ] {
            assert!(validate_name(name).is_ok(), "{name} must stay valid");
        }
    }

    #[tokio::test]
    async fn cleanup_partial_import_spares_preexisting_destination() {
        let _guard = lock_env();
        let dir = test_dir("cleanup-spares-dest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        seed_test_env(&dir);
        std::fs::write(session_path("victim"), b"original-good-session").unwrap();
        std::fs::write(sidecar_path("victim", "-journal"), b"orig-journal").unwrap();
        let stale_tmp = session_dir().join(format!("victim.session.tmp-{}", std::process::id()));
        std::fs::write(&stale_tmp, b"tmp-leftover").unwrap();

        cleanup_partial_import("victim");

        assert_eq!(
            std::fs::read(session_path("victim")).unwrap(),
            b"original-good-session",
            "pre-existing destination must survive cleanup"
        );
        assert_eq!(
            std::fs::read(sidecar_path("victim", "-journal")).unwrap(),
            b"orig-journal",
            "pre-existing sidecars must survive cleanup"
        );
        assert!(!stale_tmp.exists(), "this run's temp file must be swept");
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn corrupt_sqlite_body() -> Vec<u8> {
        let mut bytes = SQLITE_MAGIC.to_vec();
        bytes.extend(std::iter::repeat_n(0u8, 496));
        bytes[16] = 0xFF;
        bytes[17] = 0xFF;
        bytes
    }

    #[tokio::test]
    async fn failed_force_import_preserves_pre_existing_session() {
        let _guard = lock_env();
        let dir = test_dir("force-import-preserves");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            drop(held);
        }
        let original_bytes = std::fs::read(session_path("work")).unwrap();
        let original_sha = sha256_hex(&original_bytes);

        let bad = inbox.join("bad.session");
        std::fs::write(&bad, corrupt_sqlite_body()).unwrap();
        let err = import_session(&bad, Some("work"), true).await.unwrap_err();
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("not a valid session"),
            "{err}"
        );

        assert!(
            session_path("work").exists(),
            "failed --force import must not delete the pre-existing session"
        );
        let after = std::fs::read(session_path("work")).unwrap();
        assert_eq!(sha256_hex(&after), original_sha, "bytes must be untouched");
        {
            let reopened = open_session("work").await.unwrap();
            drop(reopened);
        }

        let fresh_bad = inbox.join("fresh-bad.session");
        std::fs::write(&fresh_bad, corrupt_sqlite_body()).unwrap();
        let err = import_session(&fresh_bad, Some("x"), false)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("not a valid session"),
            "{err}"
        );
        assert!(
            !session_path("x").exists(),
            "rejected fresh imports must leave nothing behind"
        );

        remove_session("work").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn export_without_out_writes_to_app_sessions_dir() {
        let _guard = lock_env();
        let dir = test_dir("export-default-dest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            drop(held);
        }
        let exported = export_session("work", None).await.unwrap();
        assert_eq!(
            exported.path.parent().map(|p| p.to_path_buf()),
            Some(session_dir()),
            "default destination must live in the app sessions dir, not CWD"
        );
        assert_eq!(exported.path.file_name().unwrap(), "work.session.export");
        let source_bytes = std::fs::read(session_path("work")).unwrap();
        assert_eq!(std::fs::read(&exported.path).unwrap(), source_bytes);
        assert_eq!(exported.sha256, sha256_hex(&source_bytes));
        remove_session("work").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn export_refuses_symlinked_destination() {
        use std::os::unix::fs::symlink;
        let _guard = lock_env();
        let dir = test_dir("export-symlink");
        let out = dir.join("out");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&out).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            drop(held);
        }
        let decoy = out.join("decoy.txt");
        std::fs::write(&decoy, b"do-not-touch").unwrap();
        let link = out.join("link.session");
        symlink(&decoy, &link).unwrap();
        let err = export_session("work", Some(&link)).await.unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        assert_eq!(
            std::fs::read(&decoy).unwrap(),
            b"do-not-touch",
            "symlink target must not be written through"
        );
        remove_session("work").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_sweeps_stale_tmp_files() {
        let _guard = lock_env();
        let dir = test_dir("remove-sweeps-tmp");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            drop(held);
        }
        let stale_a = session_dir().join("work.session.tmp-111111");
        let stale_b = session_dir().join("work.session.tmp-999999-journal");
        let foreign_tmp = session_dir().join("ghost.session.tmp-222222");
        std::fs::write(&stale_a, b"x").unwrap();
        std::fs::write(&stale_b, b"x").unwrap();
        std::fs::write(&foreign_tmp, b"x").unwrap();

        remove_session("work").await.unwrap();

        assert!(!session_path("work").exists());
        assert!(!lock_path("work").exists());
        assert!(!stale_a.exists(), "stale tmp must be swept");
        assert!(!stale_b.exists(), "stale tmp sidecars must be swept");
        assert!(
            foreign_tmp.exists(),
            "tmp files of other accounts must not be touched"
        );
        std::fs::remove_file(&foreign_tmp).unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_refuses_target_held_by_live_process() {
        let _guard = lock_env();
        let dir = test_dir("import-held-target");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("busy").await.unwrap();
            drop(held);
        }
        let backup = inbox.join("busy.session");
        export_session("busy", Some(&backup)).await.unwrap();
        {
            let live = open_session("busy").await.unwrap();
            let err = import_session(&backup, Some("busy"), true)
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("in use by another process"),
                "{err}"
            );
            drop(live);
        }
        remove_session("busy").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fixture_telethon(dc_id: i32) -> TelethonSessionData {
        TelethonSessionData {
            schema_version: 6,
            dc_id,
            server_address: "149.154.167.51".to_string(),
            port: 443,
            auth_key: [0xA7u8; 256],
            user_id: Some(8675309),
        }
    }

    #[tokio::test]
    async fn telethon_parse_reads_classic_schema() {
        let dir = test_dir("tg-parse-classic");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        let fixture = inbox.join("classic.session");
        write_telethon_fixture(&fixture, None).await;
        let parsed = parse_telethon_session(&fixture).await.unwrap();
        assert_eq!(parsed.schema_version, 0);
        assert_eq!(parsed.dc_id, 2);
        assert_eq!(parsed.server_address, "149.154.167.51");
        assert_eq!(parsed.port, 443);
        assert_eq!(parsed.auth_key, [0xA7u8; 256]);
        assert_eq!(parsed.user_id, Some(867_5309));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telethon_parse_reads_version_table_layout() {
        let dir = test_dir("tg-parse-versioned");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        let fixture = inbox.join("versioned.session");
        write_telethon_fixture(&fixture, Some(7)).await;
        let parsed = parse_telethon_session(&fixture).await.unwrap();
        assert_eq!(parsed.schema_version, 7);
        assert_eq!(parsed.dc_id, 2);
        assert_eq!(parsed.server_address, "149.154.167.51");
        assert_eq!(parsed.port, 443);
        assert_eq!(parsed.auth_key, [0xA7u8; 256]);
        assert_eq!(parsed.user_id, Some(867_5309));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telethon_parse_rejects_unknown_schema_enumerating_findings() {
        let dir = test_dir("tg-parse-unknown");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();

        let foreign = inbox.join("foreign.session");
        let db = libsql::Builder::new_local(&foreign).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("CREATE TABLE entities (id INTEGER)", ())
            .await
            .unwrap();
        conn.execute("CREATE TABLE sent_files (id INTEGER)", ())
            .await
            .unwrap();
        drop(conn);
        drop(db);
        let err = parse_telethon_session(&foreign).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("sessions"), "{msg}");
        assert!(
            msg.contains("entities") && msg.contains("sent_files"),
            "{msg}"
        );

        let partial = inbox.join("partial.session");
        let db = libsql::Builder::new_local(&partial).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("CREATE TABLE sessions (dc_id INTEGER, port INTEGER)", ())
            .await
            .unwrap();
        drop(conn);
        drop(db);
        let err = parse_telethon_session(&partial).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("auth_key"), "{msg}");
        assert!(msg.contains("server_address"), "{msg}");
        assert!(msg.contains("dc_id") && msg.contains("port"), "{msg}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telethon_parse_rejects_unusable_session_rows_honestly() {
        let dir = test_dir("tg-parse-badrows");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();

        let empty = inbox.join("empty.session");
        let db = libsql::Builder::new_local(&empty).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE sessions (dc_id INTEGER PRIMARY KEY, server_address TEXT, port INTEGER, auth_key BLOB, takeout_id BLOB, user_id INTEGER)",
            (),
        )
        .await
        .unwrap();
        drop(conn);
        drop(db);
        let err = parse_telethon_session(&empty).await.unwrap_err();
        assert!(err.to_string().contains("no row"), "{}", err);

        let nullkey = inbox.join("nullkey.session");
        let db = libsql::Builder::new_local(&nullkey).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE sessions (dc_id INTEGER PRIMARY KEY, server_address TEXT, port INTEGER, auth_key BLOB, takeout_id BLOB, user_id INTEGER)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES (2, '149.154.167.51', 443, NULL, NULL, 1)",
            (),
        )
        .await
        .unwrap();
        drop(conn);
        drop(db);
        let err = parse_telethon_session(&nullkey).await.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("auth_key"),
            "{}",
            err
        );

        let shortkey = inbox.join("shortkey.session");
        let db = libsql::Builder::new_local(&shortkey).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE sessions (dc_id INTEGER PRIMARY KEY, server_address TEXT, port INTEGER, auth_key BLOB, takeout_id BLOB, user_id INTEGER)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            &format!(
                "INSERT INTO sessions VALUES (2, '149.154.167.51', 443, X'{}', NULL, 1)",
                "B7".repeat(128)
            ),
            (),
        )
        .await
        .unwrap();
        drop(conn);
        drop(db);
        let err = parse_telethon_session(&shortkey).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("128") && msg.contains("256"), "{msg}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telethon_parse_rejects_garbage_and_missing_files() {
        let dir = test_dir("tg-parse-garbage");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        let junk = inbox.join("junk.session");
        std::fs::write(&junk, b"definitely not sqlite").unwrap();
        let err = parse_telethon_session(&junk).await.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("sqlite")
                || err.to_string().to_lowercase().contains("header"),
            "{}",
            err
        );
        let missing = inbox.join("missing.session");
        let err = parse_telethon_session(&missing).await.unwrap_err();
        assert!(err.to_string().contains("missing.session"), "{}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telethon_convert_end_to_end_roundtrips_auth_key_and_dc() {
        let _guard = lock_env();
        let dir = test_dir("tg-e2e-roundtrip");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        seed_test_env(&dir);
        let fixture = inbox.join("fromtg.session");
        write_telethon_fixture(&fixture, Some(7)).await;
        let imported = convert_telethon_session(&fixture, None, false)
            .await
            .unwrap();
        assert_eq!(imported.account, "fromtg");
        assert_eq!(imported.path, session_path("fromtg"));
        {
            let reopened = open_session("fromtg").await.unwrap();
            assert_eq!(reopened.session.home_dc_id().unwrap(), 2);
            let option = reopened.session.dc_option(2).unwrap().expect("dc option");
            assert_eq!(option.auth_key, Some([0xA7u8; 256]));
            let myself = reopened
                .session
                .peer(grammers_client::session::types::PeerId::self_user())
                .await
                .unwrap()
                .expect("cached self user");
            match myself {
                PeerInfo::User { id, .. } => assert_eq!(id, 867_5309),
                other => panic!("expected self user, got {other:?}"),
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(session_path("fromtg"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "converted session must be owner-only");
        }
        let mut failure_text = String::new();
        let mut broken = fixture_telethon(2);
        broken.server_address = "venus.web.telegram.org".to_string();
        if let Err(e) = write_native_from_telethon("x", &broken, false).await {
            failure_text = format!("{e:#}");
        }
        assert!(
            !failure_text.contains("167,") && !failure_text.contains(&"A7".repeat(256)),
            "error output leaked key material: {failure_text}"
        );
        remove_session("fromtg").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn write_telethon_fixture(path: &Path, version: Option<i64>) {
        let db = libsql::Builder::new_local(path).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE sessions (dc_id INTEGER PRIMARY KEY, server_address TEXT, port INTEGER, auth_key BLOB, takeout_id BLOB, user_id INTEGER)",
            (),
        )
        .await
        .unwrap();
        if let Some(v) = version {
            conn.execute("CREATE TABLE version (number INTEGER PRIMARY KEY)", ())
                .await
                .unwrap();
            conn.execute(&format!("INSERT INTO version VALUES ({v})"), ())
                .await
                .unwrap();
        }
        conn.execute(
            &format!(
                "INSERT INTO sessions VALUES (2, '149.154.167.51', 443, X'{}', NULL, 8675309)",
                "A7".repeat(256)
            ),
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn write_native_from_telethon_authors_working_session() {
        let _guard = lock_env();
        let dir = test_dir("telethon-author");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let data = fixture_telethon(2);
        let path = write_native_from_telethon("fromtg", &data, false)
            .await
            .unwrap();
        assert_eq!(path, session_path("fromtg"));

        {
            let reopened = open_session("fromtg").await.unwrap();
            assert_eq!(reopened.session.home_dc_id().unwrap(), 2);
            let option = reopened.session.dc_option(2).unwrap().expect("dc option");
            assert_eq!(option.auth_key, Some(data.auth_key));
            let myself = reopened
                .session
                .peer(grammers_client::session::types::PeerId::self_user())
                .await
                .unwrap()
                .expect("cached self user");
            match myself {
                PeerInfo::User {
                    id,
                    is_self: Some(true),
                    ..
                } => assert_eq!(id, 8675309),
                other => panic!("expected self user, got {other:?}"),
            }
        }

        let err = write_native_from_telethon("fromtg", &data, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--force to overwrite"), "{err}");
        write_native_from_telethon("fromtg", &data, true)
            .await
            .unwrap();

        remove_session("fromtg").await.unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_native_from_telethon_rejects_hostname_and_bad_port() {
        let _guard = lock_env();
        let dir = test_dir("telethon-badaddr");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let mut data = fixture_telethon(2);
        data.server_address = "venus.web.telegram.org".to_string();
        let err = write_native_from_telethon("x", &data, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not an IP literal"), "{err}");

        data.server_address = "149.154.167.51".to_string();
        data.port = 70_000;
        let err = write_native_from_telethon("x", &data, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid Telethon port"), "{err}");

        let mut ipv6 = fixture_telethon(2);
        ipv6.server_address = "2001:b28:f23d:f001::a".to_string();
        write_native_from_telethon("ipv6case", &ipv6, false)
            .await
            .unwrap();
        remove_session("ipv6case").await.unwrap();

        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exported_session_debug_never_contains_auth_material() {
        let exported = ExportedSession {
            account: "work".to_string(),
            path: PathBuf::from("/tmp/work.session"),
            bytes: 12_345,
            sha256: sha256_hex(b"public-bytes"),
        };
        let rendered = format!("{exported:?}");
        assert!(
            !rendered.contains("auth_key"),
            "no auth material: {rendered}"
        );
        assert_eq!(exported.sha256.len(), 64);
    }

    #[test]
    fn telethon_data_debug_masks_auth_key() {
        let data = fixture_telethon(2);
        let rendered = format!("{data:?}");
        assert!(
            rendered.contains("[REDACTED 256 bytes]"),
            "auth key must be masked: {rendered}"
        );
        assert!(
            !rendered.contains("167,"),
            "raw key bytes leaked via Debug: {rendered}"
        );
    }

    #[tokio::test]
    async fn sqlite_update_state_advances_and_resumes_next_start() {
        let _guard = lock_env();
        let dir = test_dir("state-advance-resume");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let path = session_path("work");
        {
            let s = SqliteSession::open(&path).await.unwrap();
            s.set_update_state(grammers_session::types::UpdateState::All(
                grammers_session::types::UpdatesState {
                    pts: 100,
                    qts: 20,
                    date: 1_700_000_000,
                    seq: 5,
                    channels: vec![grammers_session::types::ChannelState { id: 123, pts: 7 }],
                },
            ))
            .await
            .unwrap();
            let loaded = s.updates_state().await.unwrap();
            assert_eq!(loaded.pts, 100);
            assert_eq!(loaded.qts, 20);
            assert_eq!(loaded.date, 1_700_000_000);
            assert_eq!(loaded.seq, 5);
            assert_eq!(loaded.channels.len(), 1);
            assert_eq!(loaded.channels[0].id, 123);
            assert_eq!(loaded.channels[0].pts, 7);
        }
        {
            let s2 = SqliteSession::open(&path).await.unwrap();
            let resumed = s2.updates_state().await.unwrap();
            assert_eq!(resumed.pts, 100);
            assert_eq!(resumed.qts, 20);
            assert_eq!(resumed.channels[0].pts, 7);
            s2.set_update_state(grammers_session::types::UpdateState::Primary {
                pts: 101,
                date: 1_700_000_100,
                seq: 6,
            })
            .await
            .unwrap();
            let after = s2.updates_state().await.unwrap();
            assert_eq!(after.pts, 101);
            assert_eq!(after.date, 1_700_000_100);
            assert_eq!(after.seq, 6);
            assert_eq!(after.qts, 20);
        }
        {
            let s3 = SqliteSession::open(&path).await.unwrap();
            let final_state = s3.updates_state().await.unwrap();
            assert_eq!(final_state.pts, 101);
            assert_eq!(final_state.qts, 20);
            assert_eq!(final_state.channels[0].pts, 7);
        }
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sqlite_channel_state_batched_persist_resumes_catch_up() {
        let _guard = lock_env();
        let dir = test_dir("state-channel-batch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let path = session_path("work");
        {
            let s = SqliteSession::open(&path).await.unwrap();
            s.set_update_state(grammers_session::types::UpdateState::All(
                grammers_session::types::UpdatesState {
                    pts: 1,
                    qts: 0,
                    date: 1,
                    seq: 0,
                    channels: vec![],
                },
            ))
            .await
            .unwrap();
            s.set_update_state(grammers_session::types::UpdateState::Channel { id: 999, pts: 42 })
                .await
                .unwrap();
            s.set_update_state(grammers_session::types::UpdateState::Channel { id: 1000, pts: 7 })
                .await
                .unwrap();
            let loaded = s.updates_state().await.unwrap();
            assert_eq!(loaded.channels.len(), 2);
        }
        {
            let s2 = SqliteSession::open(&path).await.unwrap();
            let resumed = s2.updates_state().await.unwrap();
            assert!(resumed.channels.iter().any(|c| c.id == 999 && c.pts == 42));
            assert!(resumed.channels.iter().any(|c| c.id == 1000 && c.pts == 7));
            s2.set_update_state(grammers_session::types::UpdateState::Channel { id: 999, pts: 43 })
                .await
                .unwrap();
            let after = s2.updates_state().await.unwrap();
            assert!(after.channels.iter().any(|c| c.id == 999 && c.pts == 43));
        }
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sqlite_message_box_load_resumes_from_persisted_state() {
        let _guard = lock_env();
        let dir = test_dir("state-mbox-resume");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let path = session_path("work");
        {
            let s = SqliteSession::open(&path).await.unwrap();
            s.set_update_state(grammers_session::types::UpdateState::All(
                grammers_session::types::UpdatesState {
                    pts: 55,
                    qts: 9,
                    date: 12345,
                    seq: 3,
                    channels: vec![grammers_session::types::ChannelState { id: 777, pts: 11 }],
                },
            ))
            .await
            .unwrap();
        }
        {
            let s2 = SqliteSession::open(&path).await.unwrap();
            let state = s2.updates_state().await.unwrap();
            let mbox = grammers_session::updates::MessageBoxes::load(state.clone());
            assert!(!mbox.is_empty());
            let back = mbox.session_state();
            assert_eq!(back.pts, 55);
            assert_eq!(back.qts, 9);
            assert_eq!(back.date, 12345);
            assert_eq!(back.seq, 3);
            assert!(back.channels.iter().any(|c| c.id == 777 && c.pts == 11));
            let empty = grammers_session::updates::MessageBoxes::new();
            assert!(empty.is_empty());
            assert_eq!(empty.session_state().pts, 0);
        }
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
