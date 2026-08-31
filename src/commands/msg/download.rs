use crate::chat_target::ChatTarget;
use crate::client::ClientGuard;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};

use super::params::{DownloadArgs, DownloadParams};

pub(crate) fn validate_download(args: &DownloadArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    super::validate::validate_download_dir(&args.dir)?;
    if let Some(kb) = args.chunk_size_kb {
        validate_chunk_size_kb(kb)?;
    }
    Ok(())
}

pub(crate) fn download_serve_dry_run(args: &DownloadArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "id": args.id,
        "would": format!("download message {}", args.id)
    }))
}

pub(crate) async fn download(args: DownloadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_download(&args)?;
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
    shares.rate_limiter.acquire().await;
    let id = params.id;
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
        .ok_or_else(|| TeleError::Usage(format!("message {id} not found")))?;
    let name = download_name(&msg);
    super::validate::validate_download_dir(&out_dir)?;
    tokio::task::spawn_blocking({
        let out_dir = out_dir.clone();
        move || std::fs::create_dir_all(&out_dir)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
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
        return Err(TeleError::Usage("message has no media".to_string()));
    }
    let temp = download_temp_path(&path);
    tokio::task::spawn_blocking({
        let temp = temp.clone();
        move || create_download_temp(&temp)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    let ok = match chunk_size_kb {
        Some(kb) => {
            let media = msg.media().expect("media checked above");
            let mut iter = shares
                .client
                .iter_download(&media)
                .chunk_size((kb * 1024) as i32);
            let mut file = tokio::fs::File::create(&temp)
                .await
                .map_err(|e| TeleError::Other(e.to_string()))?;
            use tokio::io::AsyncWriteExt;
            loop {
                match iter.next().await {
                    Ok(Some(bytes)) => {
                        file.write_all(&bytes).await.map_err(|err| {
                            let _ = std::fs::remove_file(&temp);
                            TeleError::Other(err.to_string())
                        })?;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = std::fs::remove_file(&temp);
                        return Err(tele_invocation(e));
                    }
                }
            }
            file.sync_all()
                .await
                .map_err(|e| TeleError::Other(e.to_string()))?;
            true
        }
        None => msg.download_media(&temp).await.map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            tele_invocation(e)
        })?,
    };
    if !ok {
        let _ = std::fs::remove_file(&temp);
        return Err(TeleError::Usage("message has no media".to_string()));
    }
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
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(mtime) else {
            continue;
        };
        if age.as_secs() > 3600 {
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
