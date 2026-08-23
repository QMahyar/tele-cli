use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, SocketAddrV6};
use std::path::{Path, PathBuf};

use grammers_client::session::storages::SqliteSession;
use grammers_client::session::types::{DcOption, PeerInfo};
use grammers_client::session::Session as _;

pub const SESSION_FILE_WARNING: &str =
    "session files grant full access to their Telegram account; treat exports as secrets";

pub const TELETHON_CONVERT_BLOCKER: &str = "cannot convert Telethon sessions: reading the Telethon SQLite schema (version + sessions tables) needs a direct SQLite reader dependency (rusqlite, or libsql matching grammers-session's own engine); grammers-session 0.10 exposes no public raw-SQL access and no foreign-database ingestion, so this requires an explicit dependency approval";

pub fn session_dir() -> PathBuf {
    crate::config::app_data_dir().join("sessions")
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if matches!(name, "all" | "." | "..") {
        return Err(format!("invalid account name {name:?}: reserved"));
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
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
        }
        crate::fs_util::restrict_file_private(&path)?;
    }
    Ok(())
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

pub fn remove_session(name: &str) -> anyhow::Result<()> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    if !session_path(name).try_exists()? {
        return Ok(());
    }
    let lock = acquire_lock_file(name)?;
    let mut targets = vec![session_path(name), lock_path(name)];
    for suffix in SESSION_SIDECAR_SUFFIXES {
        targets.push(sidecar_path(name, suffix));
    }
    let result = targets
        .iter()
        .try_for_each(|path| match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        });
    drop(lock);
    result?;
    Ok(())
}

pub struct SessionLock {
    file: Option<std::fs::File>,
}

impl SessionLock {
    fn new(file: std::fs::File) -> Self {
        Self { file: Some(file) }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        drop(self.file.take());
    }
}

fn acquire_lock_file(name: &str) -> anyhow::Result<std::fs::File> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path(name))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => Err(anyhow::anyhow!(
            "session {name} is in use by another process"
        )),
        Err(e) => Err(e.into()),
    }
}

pub fn probe_live_lock(name: &str) -> anyhow::Result<()> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    acquire_lock_file(name).map(|_| ())
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
    let lock = acquire_lock_file(name)?;
    pre_restrict_sidecars(name)?;
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

pub fn export_session(name: &str, out: Option<&Path>) -> anyhow::Result<ExportedSession> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    let source = session_path(name);
    if !source.try_exists()? {
        return Err(anyhow::anyhow!(
            "no session file for account {name}: {}",
            source.display()
        ));
    }
    probe_live_lock(name)?;
    let dest = match out {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir()?.join(format!("{name}.session")),
    };
    if fs_same_file(&dest, &source) {
        return Err(anyhow::anyhow!(
            "destination equals the live session file for {name}"
        ));
    }
    std::fs::copy(&source, &dest)?;
    crate::fs_util::restrict_file_private(&dest)?;
    let bytes = std::fs::read(&dest)?;
    let size = bytes.len() as u64;
    Ok(ExportedSession {
        account: name.to_string(),
        path: dest,
        bytes: size,
        sha256: sha256_hex(&bytes),
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

fn cleanup_partial_import(name: &str) {
    let _ = std::fs::remove_file(session_path(name));
    for suffix in SESSION_SIDECAR_SUFFIXES {
        let _ = std::fs::remove_file(sidecar_path(name, suffix));
    }
}

async fn install_session_bytes(name: &str, bytes: &[u8]) -> anyhow::Result<ImportedSession> {
    if !bytes.starts_with(SQLITE_MAGIC) {
        return Err(anyhow::anyhow!(
            "not a valid session file: missing SQLite header"
        ));
    }
    crate::config::ensure_app_data_dir()?;
    crate::fs_util::create_dir_private(&session_dir())?;
    let path = session_path(name);
    pre_restrict_sidecars(name)?;
    std::fs::write(&path, bytes)?;
    crate::fs_util::restrict_file_private(&path)?;
    let probe_result = async {
        let probe = SqliteSession::open(&path).await?;
        drop(probe);
        restrict_session_files(name)?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(e) = probe_result {
        cleanup_partial_import(name);
        return Err(e.context("not a valid session file"));
    }
    Ok(ImportedSession {
        account: name.to_string(),
        path,
        bytes: bytes.len() as u64,
    })
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
    let bytes = std::fs::read(file)?;
    crate::config::ensure_app_data_dir()?;
    crate::fs_util::create_dir_private(&session_dir())?;
    let _lock = acquire_lock_file(&name)?;
    install_session_bytes(&name, &bytes).await
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

pub fn parse_telethon_session(_source: &Path) -> anyhow::Result<TelethonSessionData> {
    Err(anyhow::anyhow!("{TELETHON_CONVERT_BLOCKER}"))
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
    let lock = acquire_lock_file(name)?;
    pre_restrict_sidecars(name)?;
    std::fs::write(&path, [])?;
    crate::fs_util::restrict_file_private(&path)?;
    let session = SqliteSession::open(&path).await?;
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
    restrict_session_files(name)?;
    Ok(path)
}

pub async fn convert_telethon_session(
    source: &Path,
    as_name: Option<&str>,
    force: bool,
) -> anyhow::Result<ImportedSession> {
    let data = parse_telethon_session(source)?;
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
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for (i, k) in K.iter().enumerate() {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    async fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
        crate::config::TEST_ENV_LOCK.lock().await
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
        let _guard = lock_env().await;
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
        let _guard = lock_env().await;
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
        let second = open_session("work").await.unwrap();
        drop(second);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_session_restricts_sqlite_sidecars() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = lock_env().await;
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
                assert_eq!(mode, 0o600, "sidecar {suffix}");
            }
        }
        remove_session("work").unwrap();
        assert!(!sidecar_path("work", "-journal").exists());
        assert!(!sidecar_path("work", "-wal").exists());
        assert!(!sidecar_path("work", "-shm").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_session_rejects_concurrent_use() {
        let _guard = lock_env().await;
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
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-remove");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let held = open_session("work").await.unwrap();
        drop(held);
        remove_session("work").unwrap();
        assert!(!session_path("work").exists());
        assert!(!lock_path("work").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_rejects_in_use_session() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-remove-held");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let held = open_session("work").await.unwrap();
        let err = remove_session("work").unwrap_err();
        assert!(err.to_string().contains("is in use by another process"));
        drop(held);
        remove_session("work").unwrap();
        assert!(!session_path("work").exists());
        assert!(!lock_path("work").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_tolerates_never_created_session_file() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-remove-never-created");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        remove_session("work").unwrap();
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
        let _guard = lock_env().await;
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
        let exported = export_session("work", Some(&dest)).unwrap();
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
        remove_session("work").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn export_refuses_missing_source() {
        let _guard = lock_env().await;
        let dir = test_dir("export-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        seed_test_env(&dir);
        let err = export_session("ghost", None).unwrap_err();
        assert!(err.to_string().contains("no session file"), "{err}");
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn export_refuses_live_locked_source() {
        let _guard = lock_env().await;
        let dir = test_dir("export-locked");
        let out = dir.join("out");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&out).unwrap();
        seed_test_env(&dir);
        {
            let held = open_session("work").await.unwrap();
            let err = export_session("work", Some(&out.join("w.session"))).unwrap_err();
            assert!(
                err.to_string().contains("in use by another process"),
                "{err}"
            );
            assert!(!out.join("w.session").exists(), "nothing may be written");
            drop(held);
        }
        export_session("work", Some(&out.join("w.session"))).unwrap();
        remove_session("work").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn sqlite_scaffold(path: &Path) {
        let s = SqliteSession::open(path).await.unwrap();
        drop(s);
    }

    #[tokio::test]
    async fn import_export_roundtrip_is_byte_identical() {
        let _guard = lock_env().await;
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
        let first = export_session("origin", Some(&backup)).unwrap();
        let imported = import_session(&backup, Some("restored"), false)
            .await
            .unwrap();
        assert_eq!(imported.account, "restored");
        assert_eq!(imported.bytes, first.bytes);
        let reexport_path = dir.join("reexport.session");
        let reexport = export_session("restored", Some(&reexport_path)).unwrap();
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

        remove_session("restored").unwrap();
        remove_session("origin").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_derives_and_validates_name_from_stem() {
        let _guard = lock_env().await;
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

        remove_session("team_a").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn import_rejects_non_sqlite_garbage() {
        let _guard = lock_env().await;
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

    #[tokio::test]
    async fn import_refuses_target_held_by_live_process() {
        let _guard = lock_env().await;
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
        export_session("busy", Some(&backup)).unwrap();
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
        remove_session("busy").unwrap();
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
    async fn telethon_parse_reports_dependency_blocker() {
        let _guard = lock_env().await;
        let dir = test_dir("telethon-blocker");
        let inbox = dir.join("inbox");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&inbox).unwrap();
        seed_test_env(&dir);
        let fake = inbox.join("t.session");
        sqlite_scaffold(&fake).await;
        let err = convert_telethon_session(&fake, Some("fromtg"), false)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("rusqlite") && msg.contains("libsql"), "{msg}");
        assert!(
            msg.contains("dependency approval"),
            "must name the required approval: {msg}"
        );
        assert!(
            !session_path("fromtg").exists(),
            "blocked conversion must not create a session file"
        );
        assert!(
            !lock_path("fromtg").exists(),
            "blocked conversion must not create a lock file"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_native_from_telethon_authors_working_session() {
        let _guard = lock_env().await;
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

        remove_session("fromtg").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_native_from_telethon_rejects_hostname_and_bad_port() {
        let _guard = lock_env().await;
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
        remove_session("ipv6case").unwrap();

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
}
