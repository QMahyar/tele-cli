use crate::client::ClientGuard;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{require_explicit_selection, run_fanout, GlobalFlags};

use super::params::{DownloadArgs, DownloadParams};

pub(crate) fn parse_download_date(
    flag: &str,
    value: &str,
) -> TeleResult<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = crate::commands::parse_unixtime(value) {
        return Ok(dt);
    }
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc())
        .ok_or_else(|| {
            TeleError::Usage(format!(
                "invalid {flag} {value:?}: use RFC 3339, a Unix timestamp, or YYYY-MM-DD"
            ))
        })
}

pub(crate) fn validate_download(args: &DownloadArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    super::validate::validate_download_dir(&args.dir)?;
    if let Some(kb) = args.chunk_size_kb {
        validate_chunk_size_kb(kb)?;
    }
    if args.all && args.id.is_some() {
        return Err(TeleError::Usage(
            "--all and --id are mutually exclusive".to_string(),
        ));
    }
    if args.album && args.id.is_none() {
        return Err(TeleError::Usage("--album requires --id".to_string()));
    }
    if !args.all && (args.since.is_some() || args.until.is_some()) {
        return Err(TeleError::Usage(
            "--since/--until require --all".to_string(),
        ));
    }
    if !args.all && args.id.is_none() {
        return Err(TeleError::Usage(
            "--id required unless --all is used".to_string(),
        ));
    }
    let since = match &args.since {
        Some(v) => Some(parse_download_date("--since", v)?),
        None => None,
    };
    let until = match &args.until {
        Some(v) => Some(parse_download_date("--until", v)?),
        None => None,
    };
    if let (Some(s), Some(u)) = (since, until) {
        if s > u {
            return Err(TeleError::Usage(
                "--since must not be after --until".to_string(),
            ));
        }
    }
    if let Some(limit) = args.limit {
        let max = u32::try_from(limit)
            .map_err(|_| TeleError::Usage(format!("--limit too large: {limit}")))?;
        crate::commands::validate_limit(max, 1_000_000, "limit")?;
    }
    Ok(())
}

pub(crate) fn download_serve_dry_run(args: &DownloadArgs) -> TeleResult<serde_json::Value> {
    let would = if args.all {
        "download all media messages in chat history".to_string()
    } else if args.album {
        format!(
            "download album containing message {}",
            args.id.unwrap_or_default()
        )
    } else {
        format!("download message {}", args.id.unwrap_or_default())
    };
    Ok(serde_json::json!({
        "dry_run": true,
        "id": args.id,
        "all": args.all,
        "album": args.album,
        "since": args.since,
        "until": args.until,
        "limit": args.limit,
        "would": would
    }))
}

pub(crate) async fn download(args: DownloadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_download(&args)?;
    require_explicit_selection("msg download", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return download_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            crate::client::authorize(&guard.client).await?;
            download_core(&guard.shares(), DownloadParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn download_core(
    shares: &crate::client::ServeShares,
    params: DownloadParams,
) -> TeleResult<serde_json::Value> {
    if params.all {
        return download_bulk_core(shares, params).await;
    }
    shares.rate_limiter.acquire().await;
    let id = params
        .id
        .ok_or_else(|| TeleError::Usage("--id required unless --all is used".to_string()))?;
    let out_dir = params.dir.clone();
    let chunk_size_kb = params.chunk_size_kb;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let found = shares
        .client
        .get_messages_by_id(chat_ref, &[id])
        .await
        .map_err(tele_invocation)?;
    let msg = found
        .into_iter()
        .flatten()
        .next()
        .ok_or_else(|| TeleError::Invocation(format!("message {id} not found"), None))?;
    let name = download_name(&msg);
    super::validate::validate_download_dir(&out_dir)?;
    if params.album {
        let grouped_id = msg.grouped_id().ok_or_else(|| {
            TeleError::Invocation(format!("message {id} is not part of an album"), None)
        })?;
        ensure_download_dir(&out_dir).await?;
        return download_album_core(shares, chat_ref, id, grouped_id, &params).await;
    }
    ensure_download_dir(&out_dir).await?;
    let path = std::path::Path::new(&out_dir).join(name);
    if !params.force {
        refuse_existing_download_target(&path)?;
    }
    tokio::task::spawn_blocking({
        let path = path.clone();
        move || sweep_stale_download_temps(&path)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))?;
    if msg.media().is_none() {
        return Err(TeleError::Invocation(
            "message has no media".to_string(),
            None,
        ));
    }
    let temp = download_temp_path(&path);
    tokio::task::spawn_blocking({
        let temp = temp.clone();
        move || create_download_temp(&temp)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    stream_media_to_file(shares, &msg, &temp, chunk_size_kb).await?;
    tokio::task::spawn_blocking({
        let temp = temp.clone();
        let path = path.clone();
        move || commit_download(&temp, &path)
    })
    .await
    .map_err(|e| TeleError::Other(format!("download commit task failed: {e}")))??;
    let bytes = tokio::task::spawn_blocking({
        let path = path.clone();
        move || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))?;
    Ok(serde_json::json!({"path": path.to_string_lossy(), "bytes": bytes}))
}

async fn ensure_download_dir(out_dir: &str) -> TeleResult<()> {
    tokio::task::spawn_blocking({
        let out_dir = out_dir.to_string();
        move || std::fs::create_dir_all(&out_dir)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))?
    .map_err(|e| TeleError::Other(e.to_string()))
}

async fn stream_media_to_file(
    shares: &crate::client::ServeShares,
    msg: &grammers_client::message::Message,
    temp: &std::path::Path,
    chunk_size_kb: Option<usize>,
) -> TeleResult<()> {
    match chunk_size_kb {
        Some(kb) => {
            let media = msg
                .media()
                .ok_or_else(|| TeleError::Invocation("message has no media".to_string(), None))?;
            let mut iter = shares
                .client
                .iter_download(&media)
                .chunk_size((kb * 1024) as i32);
            let mut file = tokio::fs::File::create(temp)
                .await
                .map_err(|e| TeleError::Other(e.to_string()))?;
            use tokio::io::AsyncWriteExt;
            loop {
                match iter.next().await {
                    Ok(Some(bytes)) => {
                        file.write_all(&bytes).await.map_err(|err| {
                            let _ = std::fs::remove_file(temp);
                            TeleError::Other(err.to_string())
                        })?;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = std::fs::remove_file(temp);
                        return Err(tele_invocation(e));
                    }
                }
            }
            file.sync_all()
                .await
                .map_err(|e| TeleError::Other(e.to_string()))?;
        }
        None => {
            let ok = msg.download_media(temp).await.map_err(|e| {
                let _ = std::fs::remove_file(temp);
                tele_invocation(e)
            })?;
            if !ok {
                let _ = std::fs::remove_file(temp);
                return Err(TeleError::Invocation(
                    "message has no media".to_string(),
                    None,
                ));
            }
        }
    }
    Ok(())
}

struct BulkDownloaded {
    id: i32,
    path: std::path::PathBuf,
    bytes: u64,
}

async fn bulk_download_one(
    shares: &crate::client::ServeShares,
    msg: &grammers_client::message::Message,
    out_dir: &std::path::Path,
    chunk_size_kb: Option<usize>,
    force: bool,
) -> TeleResult<Option<BulkDownloaded>> {
    if msg.media().is_none() {
        return Ok(None);
    }
    let path = out_dir.join(bulk_media_name(&download_name(msg), msg.id()));
    if !force && tokio::fs::metadata(&path).await.is_ok() {
        return Ok(None);
    }
    let temp = download_temp_path(&path);
    tokio::task::spawn_blocking({
        let temp = temp.clone();
        move || create_download_temp(&temp)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    stream_media_to_file(shares, msg, &temp, chunk_size_kb).await?;
    tokio::task::spawn_blocking({
        let temp = temp.clone();
        let path = path.clone();
        move || commit_download(&temp, &path)
    })
    .await
    .map_err(|e| TeleError::Other(format!("download commit task failed: {e}")))??;
    let bytes = tokio::task::spawn_blocking({
        let path = path.clone();
        move || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))?;
    Ok(Some(BulkDownloaded {
        id: msg.id(),
        path,
        bytes,
    }))
}

pub(crate) fn bulk_media_name(base: &str, id: i32) -> String {
    match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{id}.{ext}"),
        _ => format!("{base}-{id}"),
    }
}

pub(crate) fn checkpoint_path(out_dir: &std::path::Path, chat_id: i64) -> std::path::PathBuf {
    out_dir.join(format!(".telecli-download-{chat_id}.json"))
}

pub(crate) async fn load_checkpoint(path: &std::path::Path) -> Option<i32> {
    let raw = tokio::fs::read_to_string(path).await.ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let id = value.get("last_message_id")?.as_i64()?;
    i32::try_from(id).ok()
}

pub(crate) async fn save_checkpoint(path: &std::path::Path, chat_id: i64, last_message_id: i32) {
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "last_message_id": last_message_id,
    });
    let _ = tokio::fs::write(path, payload.to_string()).await;
}

pub(crate) async fn download_bulk_core(
    shares: &crate::client::ServeShares,
    params: DownloadParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let out_dir = std::path::PathBuf::from(&params.dir);
    super::validate::validate_download_dir(&params.dir)?;
    ensure_download_dir(&params.dir).await?;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let chat_id = chat.id().bare_id().unwrap_or_default();
    let since = match &params.since {
        Some(v) => Some(parse_download_date("--since", v)?),
        None => None,
    };
    let until = match &params.until {
        Some(v) => Some(parse_download_date("--until", v)?),
        None => None,
    };
    let scan_limit = params.limit.unwrap_or(1000);
    let state_path = checkpoint_path(&out_dir, chat_id);
    let resume_from = load_checkpoint(&state_path).await;
    let mut iter = shares.client.iter_messages(chat_ref).limit(scan_limit);
    let mut files: Vec<serde_json::Value> = Vec::new();
    let mut skipped_existing = 0usize;
    let mut scanned = 0usize;
    let mut served = 0usize;
    while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
        scanned += 1;
        served += 1;
        shares.rate_limiter.acquire_for_items(served).await;
        if resume_from.is_some_and(|last| msg.id() >= last) {
            continue;
        }
        let date = msg.date();
        if until.is_some_and(|u| date.timestamp() > u.timestamp()) {
            continue;
        }
        if since.is_some_and(|s| date.timestamp() < s.timestamp()) {
            break;
        }
        match bulk_download_one(shares, &msg, &out_dir, params.chunk_size_kb, params.force).await? {
            Some(entry) => {
                save_checkpoint(&state_path, chat_id, msg.id()).await;
                files.push(serde_json::json!({
                    "id": entry.id,
                    "path": entry.path.to_string_lossy(),
                    "bytes": entry.bytes,
                }));
            }
            None => {
                if msg.media().is_some() {
                    skipped_existing += 1;
                }
            }
        }
    }
    Ok(serde_json::json!({
        "all": true,
        "chat": params.chat,
        "scanned": scanned,
        "downloaded": files.len(),
        "skipped_existing": skipped_existing,
        "resumed_from": resume_from,
        "checkpoint": state_path.to_string_lossy(),
        "files": files,
    }))
}

async fn download_album_core(
    shares: &crate::client::ServeShares,
    chat_ref: grammers_session::types::PeerRef,
    anchor: i32,
    grouped_id: i64,
    params: &DownloadParams,
) -> TeleResult<serde_json::Value> {
    let out_dir = std::path::PathBuf::from(&params.dir);
    let mut iter = shares
        .client
        .iter_messages(chat_ref)
        .offset_id(anchor.saturating_add(10))
        .limit(20);
    let mut files: Vec<serde_json::Value> = Vec::new();
    let mut skipped_existing = 0usize;
    let mut served = 0usize;
    while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
        if msg.grouped_id() != Some(grouped_id) {
            continue;
        }
        served += 1;
        shares.rate_limiter.acquire_for_items(served).await;
        match bulk_download_one(shares, &msg, &out_dir, params.chunk_size_kb, params.force).await? {
            Some(entry) => files.push(serde_json::json!({
                "id": entry.id,
                "path": entry.path.to_string_lossy(),
                "bytes": entry.bytes,
            })),
            None => skipped_existing += 1,
        }
    }
    Ok(serde_json::json!({
        "album": true,
        "grouped_id": grouped_id,
        "anchor": anchor,
        "downloaded": files.len(),
        "skipped_existing": skipped_existing,
        "files": files,
    }))
}

pub(crate) fn validate_chunk_size_kb(kb: usize) -> TeleResult<()> {
    if !(4..=512).contains(&kb) || !kb.is_multiple_of(4) {
        return Err(TeleError::Usage(
            "--chunk-size-kb must be 4-512 and a multiple of 4".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn download_name(msg: &grammers_client::message::Message) -> String {
    use grammers_client::media::Media;
    let name = match msg.media() {
        Some(Media::Document(d)) => d.name().map(str::to_string).unwrap_or_default(),
        Some(Media::Photo(_)) => "photo.jpg".to_string(),
        Some(Media::Sticker(_)) => "sticker.webp".to_string(),
        Some(_) => "media.bin".to_string(),
        None => "media.bin".to_string(),
    };
    sanitize_download_name(&name)
}

pub(crate) fn sanitize_download_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out = String::with_capacity(base.len());
    for c in base.chars() {
        out.push(match c {
            '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        });
    }
    let trimmed = out.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        "document.bin".to_string()
    } else if super::validate::validate_filename(trimmed).is_err() {
        let stem = trimmed.split('.').next().unwrap_or(trimmed);
        let lower_stem = stem.to_ascii_lowercase();
        let is_reserved = matches!(lower_stem.as_str(), "con" | "prn" | "aux" | "nul")
            || (lower_stem.len() == 4
                && (lower_stem.starts_with("com") || lower_stem.starts_with("lpt"))
                && lower_stem
                    .as_bytes()
                    .get(3)
                    .is_some_and(|b| (b'1'..=b'9').contains(b)));
        if is_reserved {
            format!("_{trimmed}")
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn download_temp_path(final_path: &std::path::Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let ctr = CTR.fetch_add(1, Ordering::SeqCst);
    let name = final_path.file_name().unwrap_or_default().to_string_lossy();
    final_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".{name}.part-{}-{nanos}-{ctr}", std::process::id()))
}

pub(crate) fn sweep_stale_download_temps(final_path: &std::path::Path) {
    let Some(parent) = final_path.parent() else {
        return;
    };
    let Some(fname) = final_path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!(".{fname}.part-");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let pid_portion = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split('-').next())
            .and_then(|p| p.parse::<u32>().ok());
        if pid_portion == Some(std::process::id()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(mtime) else {
            continue;
        };
        if age.as_secs() > 86_400 {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub(crate) fn refuse_existing_download_target(path: &std::path::Path) -> TeleResult<()> {
    if path.exists() {
        return Err(TeleError::Usage(format!(
            "download target exists: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn create_download_temp(temp: &std::path::Path) -> TeleResult<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)
        .map(|_| ())
        .map_err(|e| TeleError::Other(format!("cannot create temp file {}: {e}", temp.display())))
}

pub(crate) fn commit_download(
    temp: &std::path::Path,
    final_path: &std::path::Path,
) -> TeleResult<()> {
    match std::fs::rename(temp, final_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(temp);
            Err(TeleError::Other(format!(
                "cannot move download into place at {}: {e}",
                final_path.display()
            )))
        }
    }
}
