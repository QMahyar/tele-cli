use crate::error::{TeleError, TeleResult};
use crate::fs_util::{path_under_guard, resolve_for_guard};

pub(crate) fn is_reserved_device_name(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    if matches!(lower.as_str(), "con" | "prn" | "aux" | "nul") {
        return true;
    }
    lower.len() == 4
        && (lower.starts_with("com") || lower.starts_with("lpt"))
        && lower
            .as_bytes()
            .get(3)
            .is_some_and(|b| (b'1'..=b'9').contains(b))
}

pub(crate) fn validate_filename(name: &str) -> TeleResult<()> {
    if name.is_empty() {
        return Err(TeleError::Usage("file name must not be empty".to_string()));
    }
    if name.ends_with([' ', '.']) {
        return Err(TeleError::Usage(format!(
            "refusing to upload file {name:?}: name ends with a character Windows would strip"
        )));
    }
    if name.contains(':') {
        return Err(TeleError::Usage(format!(
            "refusing to upload file {name:?}: ':' is not allowed in a Windows file name"
        )));
    }
    let stem = name.split('.').next().unwrap_or(name);
    if is_reserved_device_name(stem) {
        return Err(TeleError::Usage(format!(
            "refusing to upload file {name:?}: reserved Windows device name"
        )));
    }
    Ok(())
}

pub(crate) const MAX_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) fn check_upload_size(bytes: u64) -> TeleResult<()> {
    if bytes > MAX_UPLOAD_BYTES {
        return Err(TeleError::Usage(format!(
            "refusing to upload file larger than 2 GiB (got {bytes} bytes)"
        )));
    }
    Ok(())
}

pub fn validate_upload_path(path: &str) -> TeleResult<()> {
    validate_upload_path_inner(path, false)
}

pub(crate) fn validate_upload_path_inner(path: &str, dry_run: bool) -> TeleResult<()> {
    if path == "-" {
        // stdin upload: no on-disk path checks; --file-size/--file-name are
        // validated by validate_send.
        return Ok(());
    }
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    validate_filename(base)?;
    let app_data = crate::config::app_data_dir_checked()?;
    let app_dir = canonical_guard_path(&app_data.to_string_lossy());
    let canonical = canonical_guard_path(path);
    if path_under_guard(&canonical, &app_dir) {
        return Err(TeleError::Usage(
            "refusing to upload a file from the telecli app data directory".to_string(),
        ));
    }
    let lower = base.to_lowercase();
    if is_sensitive_basename(&lower) {
        return Err(TeleError::Usage(format!(
            "refusing to upload sensitive file {base}"
        )));
    }
    if !dry_run {
        let path = std::path::Path::new(path);
        if !path.is_file() {
            return Err(TeleError::Usage(format!("upload file not found: {path:?}")));
        }
        check_upload_size(std::fs::metadata(path)?.len())?;
    }
    Ok(())
}

pub(crate) const SENSITIVE_SUFFIXES: [&str; 11] = [
    ".session",
    ".session-journal",
    ".session-wal",
    ".session-shm",
    ".session.export",
    ".session.tmp",
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".kdbx",
];
pub(crate) const SENSITIVE_PREFIXES: [&str; 6] = [
    ".env",
    "config.toml",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
];
pub(crate) const SENSITIVE_EXACT: [&str; 3] = [".netrc", ".git-credentials", "credentials"];

pub fn is_sensitive_basename(lower: &str) -> bool {
    if SENSITIVE_SUFFIXES.iter().any(|s| lower.ends_with(s))
        || SENSITIVE_PREFIXES.iter().any(|s| lower.starts_with(s))
        || SENSITIVE_EXACT.contains(&lower)
    {
        return true;
    }
    let stem = lower
        .strip_suffix(".bak")
        .or_else(|| lower.strip_suffix(".backup"))
        .unwrap_or(lower);
    if stem != lower {
        return is_sensitive_basename(stem);
    }
    lower.contains(".env")
        || lower.contains("credentials")
        || lower.contains("id_rsa")
        || lower.contains("id_ed25519")
}

pub(crate) fn validate_export_out(out: &str) -> TeleResult<()> {
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return Err(TeleError::Usage("--out must name a file".to_string()));
    }
    let lower = std::path::Path::new(trimmed)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if is_sensitive_basename(&lower) {
        return Err(TeleError::Usage(
            "refusing to write --out over a sensitive file (session, key, or credentials)"
                .to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_download_dir(dir: &str) -> TeleResult<()> {
    let app_data = crate::config::app_data_dir_checked()?;
    let app_dir = canonical_guard_path(&app_data.to_string_lossy());
    let sessions_dir = canonical_guard_path(&app_data.join("sessions").to_string_lossy());
    let canonical = canonical_guard_path(dir);
    if path_under_guard(&canonical, &app_dir) || path_under_guard(&canonical, &sessions_dir) {
        return Err(TeleError::Usage(
            "refusing to download into the telecli app data directory".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn canonical_guard_path(path: &str) -> std::path::PathBuf {
    resolve_for_guard(std::path::Path::new(path))
}

pub(crate) fn validate_markdown(text: &str) -> TeleResult<()> {
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find("tg://user?id=") {
        let abs = search_from + pos;
        let prev = text[..abs].chars().last();
        let genuine = match prev {
            None => true,
            Some(c) => c == '(' || c == '<' || c.is_whitespace(),
        };
        if !genuine {
            search_from = abs + "tg://user?id=".len();
            continue;
        }
        let after = &text[abs + "tg://user?id=".len()..];
        let id = mention_id(after, prev == Some('<'));
        if !is_valid_mention_id(id) {
            return Err(TeleError::Usage(format!(
                "invalid tg://user?id= mention in --text: id must be a positive number (got {id:?})"
            )));
        }
        search_from = abs + "tg://user?id=".len();
    }
    Ok(())
}

fn mention_id(after: &str, raw_form: bool) -> &str {
    if let Some(rest) = after.strip_prefix('<') {
        return rest.split('>').next().unwrap_or("");
    }
    if raw_form {
        return after.split('>').next().unwrap_or("");
    }
    let mut end = 0;
    let mut depth = 0i32;
    for (i, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => break,
            ')' => depth -= 1,
            c if c.is_whitespace() => break,
            _ => {}
        }
        end = i + c.len_utf8();
    }
    &after[..end]
}

fn is_valid_mention_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_digit())
        && matches!(id.parse::<i64>(), Ok(v) if v > 0)
}
