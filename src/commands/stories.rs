use clap::{Args, Subcommand};
use grammers_client::tl;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::msg::validate_upload_path;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{require_explicit_selection, run_fanout, GlobalFlags};
use crate::output;

const STORY_COLUMNS: [&str; 7] = [
    "id",
    "state",
    "date",
    "expire_date",
    "caption",
    "pinned",
    "views",
];

const STORY_PERIODS: [i32; 4] = [21_600, 43_200, 86_400, 172_800];

#[derive(Subcommand)]
pub enum StoryCmd {
    Send(SendArgs),
    List(ListArgs),
    Read(ReadArgs),
    Delete(DeleteArgs),
    Pin(PinArgs),
    Unpin(PinArgs),
}

#[derive(Clone, Args)]
pub struct SendArgs {
    #[arg(long, help = "target peer: @user, t.me link, numeric id, me, +phone")]
    chat: String,
    #[arg(long, help = "path of the photo or video to post as a story")]
    file: String,
    #[arg(long, help = "optional story caption")]
    caption: Option<String>,
    #[arg(
        long,
        default_value_t = "contacts".to_string(),
        help = "audience: everyone | contacts | close-friends"
    )]
    privacy: String,
    #[arg(long, help = "also mark the story pinned")]
    pinned: bool,
    #[arg(long, help = "disallow forwarding and saving of this story")]
    noforwards: bool,
    #[arg(
        long,
        help = "story lifetime in seconds: 21600 | 43200 | 86400 | 172800 (default server-chosen)"
    )]
    period: Option<i32>,
}

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, help = "target peer: @user, t.me link, numeric id, me, +phone")]
    chat: String,
    #[arg(long, help = "read archived stories instead of active ones")]
    archive: bool,
    #[arg(
        long,
        conflicts_with = "archive",
        help = "read pinned stories instead of active ones"
    )]
    pinned: bool,
    #[arg(long, default_value_t = 50, help = "max stories to list (1-100)")]
    limit: u32,
}

#[derive(Args)]
pub struct ReadArgs {
    #[arg(long, help = "target peer: @user, t.me link, numeric id, me, +phone")]
    chat: String,
    #[arg(
        long,
        help = "mark every story of the peer up to this story id as read"
    )]
    max_id: i32,
}

#[derive(Args)]
pub struct DeleteArgs {
    #[arg(long, help = "target peer: @user, t.me link, numeric id, me, +phone")]
    chat: String,
    #[arg(long, help = "comma-separated story ids to delete, e.g. 1,2,3")]
    ids: String,
}

#[derive(Args)]
pub struct PinArgs {
    #[arg(long, help = "target peer: @user, t.me link, numeric id, me, +phone")]
    chat: String,
    #[arg(long, help = "comma-separated story ids, e.g. 1,2,3")]
    ids: String,
}

pub async fn run(cmd: StoryCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        StoryCmd::Send(a) => send(a, flags).await,
        StoryCmd::List(a) => list(a, flags).await,
        StoryCmd::Read(a) => read(a, flags).await,
        StoryCmd::Delete(a) => delete(a, flags).await,
        StoryCmd::Pin(a) => toggle_pinned(a, true, flags).await,
        StoryCmd::Unpin(a) => toggle_pinned(a, false, flags).await,
    }
}

fn parse_ids(raw: &str) -> TeleResult<Vec<i32>> {
    if raw.trim().is_empty() {
        return Err(TeleError::Usage(
            "--ids must not be empty; pass comma-separated positive story ids".to_string(),
        ));
    }
    let mut out = Vec::new();
    for part in raw.split(',') {
        let Ok(id) = part.trim().parse::<i32>() else {
            return Err(TeleError::Usage(format!(
                "--ids \"{raw}\" must be comma-separated integers"
            )));
        };
        if id <= 0 {
            return Err(TeleError::Usage(format!(
                "--ids \"{raw}\" must contain positive story ids only (got {id})"
            )));
        }
        out.push(id);
    }
    if out.is_empty() {
        return Err(TeleError::Usage(
            "--ids must not be empty; pass comma-separated positive story ids".to_string(),
        ));
    }
    Ok(out)
}

fn validate_privacy(value: &str) -> TeleResult<()> {
    match value {
        "everyone" | "contacts" | "close-friends" => Ok(()),
        _ => Err(TeleError::Usage(format!(
            "--privacy \"{value}\" must be everyone, contacts or close-friends"
        ))),
    }
}

fn validate_period(period: Option<i32>) -> TeleResult<()> {
    if let Some(p) = period {
        if !STORY_PERIODS.contains(&p) {
            return Err(TeleError::Usage(format!(
                "--period {p} must be one of 21600, 43200, 86400 or 172800 seconds"
            )));
        }
    }
    Ok(())
}

fn privacy_rules(value: &str) -> Vec<tl::enums::InputPrivacyRule> {
    match value {
        "everyone" => vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll],
        "close-friends" => vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowCloseFriends],
        _ => vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts],
    }
}

fn validate_send_args(args: &SendArgs) -> TeleResult<()> {
    crate::commands::require_chat_target(&args.chat, "chat")?;
    if args.file.trim().is_empty() {
        return Err(TeleError::Usage("--file must not be empty".to_string()));
    }
    if args.caption.as_deref().is_some_and(|c| c.trim().is_empty()) {
        return Err(TeleError::Usage("--caption must not be empty".to_string()));
    }
    validate_privacy(&args.privacy)?;
    validate_period(args.period)
}

fn looks_like_image(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "heic")
}

fn mime_type_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" | "mov" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn input_media(path: &str, file: tl::enums::InputFile) -> tl::enums::InputMedia {
    if looks_like_image(path) {
        tl::enums::InputMedia::UploadedPhoto(tl::types::InputMediaUploadedPhoto {
            spoiler: false,
            live_photo: false,
            file,
            stickers: None,
            ttl_seconds: None,
            video: None,
        })
    } else {
        tl::enums::InputMedia::UploadedDocument(tl::types::InputMediaUploadedDocument {
            nosound_video: false,
            force_file: false,
            spoiler: false,
            file,
            thumb: None,
            mime_type: mime_type_for(path).to_string(),
            attributes: Vec::new(),
            stickers: None,
            video_cover: None,
            video_timestamp: None,
            ttl_seconds: None,
        })
    }
}

fn story_random_id() -> i64 {
    static LAST_ID: AtomicI64 = AtomicI64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(1);
    LAST_ID.fetch_max(now.max(1), Ordering::SeqCst);
    LAST_ID.fetch_add(1, Ordering::SeqCst)
}

fn upload_error(e: std::io::Error) -> TeleError {
    let invocation = e
        .get_ref()
        .and_then(|s| s.downcast_ref::<grammers_client::InvocationError>());
    match invocation {
        Some(grammers_client::InvocationError::Rpc(rpc)) if rpc.code == 420 => {
            TeleError::Rpc(rpc.to_string(), rpc.code, rpc.name.clone(), rpc.value)
        }
        _ => TeleError::Other(e.to_string()),
    }
}

async fn resolve_input_peer(guard: &ClientGuard, target: &str) -> TeleResult<tl::enums::InputPeer> {
    let peer = crate::entities::resolve_peer(&guard.client, guard.session.as_ref(), target).await?;
    crate::entities::input_peer(&peer)
        .await
        .map_err(tele_invocation)
}

fn media_kind_raw(media: &tl::enums::MessageMedia) -> &'static str {
    match media {
        tl::enums::MessageMedia::Photo(_) => "photo",
        tl::enums::MessageMedia::Geo(_) => "geo",
        tl::enums::MessageMedia::GeoLive(_) => "geo_live",
        tl::enums::MessageMedia::Contact(_) => "contact",
        tl::enums::MessageMedia::Unsupported => "unsupported",
        tl::enums::MessageMedia::Document(_) => "document",
        tl::enums::MessageMedia::WebPage(_) => "webpage",
        tl::enums::MessageMedia::Venue(_) => "venue",
        tl::enums::MessageMedia::Game(_) => "game",
        tl::enums::MessageMedia::Invoice(_) => "invoice",
        tl::enums::MessageMedia::Poll(_) => "poll",
        tl::enums::MessageMedia::Dice(_) => "dice",
        tl::enums::MessageMedia::Story(_) => "story",
        tl::enums::MessageMedia::Giveaway(_) => "giveaway",
        tl::enums::MessageMedia::GiveawayResults(_) => "giveaway_results",
        tl::enums::MessageMedia::PaidMedia(_) => "paid_media",
        tl::enums::MessageMedia::ToDo(_) => "todo",
        tl::enums::MessageMedia::VideoStream(_) => "video_stream",
        tl::enums::MessageMedia::Empty => "empty",
    }
}

fn views_row(views: &tl::enums::StoryViews) -> serde_json::Value {
    let tl::enums::StoryViews::Views(v) = views;
    serde_json::json!({
        "views_count": v.views_count,
        "forwards_count": v.forwards_count,
        "reactions_count": v.reactions_count,
    })
}

fn story_item_id(item: &tl::enums::StoryItem) -> i32 {
    match item {
        tl::enums::StoryItem::Deleted(d) => d.id,
        tl::enums::StoryItem::Skipped(s) => s.id,
        tl::enums::StoryItem::Item(i) => i.id,
    }
}

fn story_row(item: &tl::enums::StoryItem) -> serde_json::Value {
    match item {
        tl::enums::StoryItem::Deleted(d) => serde_json::json!({
            "id": d.id,
            "deleted": true,
        }),
        tl::enums::StoryItem::Skipped(s) => serde_json::json!({
            "id": s.id,
            "date": s.date,
            "expire_date": s.expire_date,
            "skipped": true,
            "close_friends": s.close_friends,
            "live": s.live,
        }),
        tl::enums::StoryItem::Item(i) => {
            let mut row = serde_json::json!({
                "id": i.id,
                "date": i.date,
                "expire_date": i.expire_date,
                "caption": i.caption,
                "pinned": i.pinned,
                "public": i.public,
                "close_friends": i.close_friends,
                "noforwards": i.noforwards,
                "out": i.out,
                "edited": i.edited,
                "media_kind": media_kind_raw(&i.media),
            });
            if let Some(v) = &i.views {
                row["views"] = views_row(v);
            }
            row
        }
    }
}

fn truncate_caption(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}…")
}

fn table_row(row: &serde_json::Value) -> Vec<String> {
    let state = if row["deleted"].as_bool().unwrap_or(false) {
        "deleted"
    } else if row["skipped"].as_bool().unwrap_or(false) {
        "skipped"
    } else {
        "active"
    };
    let views = row["views"]["views_count"]
        .as_i64()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());
    vec![
        row["id"].to_string(),
        state.to_string(),
        row["date"].to_string(),
        row["expire_date"].to_string(),
        truncate_caption(row["caption"].as_str().unwrap_or_default(), 40),
        row["pinned"].as_bool().unwrap_or(false).to_string(),
        views,
    ]
}

fn sent_story_ids(updates: &tl::enums::Updates) -> Vec<i32> {
    let list = match updates {
        tl::enums::Updates::Updates(u) => Some(&u.updates),
        tl::enums::Updates::Combined(u) => Some(&u.updates),
        _ => None,
    };
    let Some(list) = list else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|u| match u {
            tl::enums::Update::Story(s) => Some(story_item_id(&s.story)),
            _ => None,
        })
        .collect()
}

fn joined(ids: &[i32]) -> String {
    ids.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn send_dry_run_payload(args: &SendArgs) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "file": args.file,
        "caption": args.caption,
        "privacy": args.privacy,
        "pinned": args.pinned,
        "noforwards": args.noforwards,
        "period": args.period,
        "would": format!("send story {} to {}", args.file, args.chat),
    })
}

fn list_dry_run_payload(chat: &str, mode: &str, limit: u32) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "mode": mode,
        "limit": limit,
        "would": format!("list {mode} stories of {chat}"),
    })
}

fn read_dry_run_payload(chat: &str, max_id: i32) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "max_id": max_id,
        "would": format!("mark stories up to {max_id} as read for {chat}"),
    })
}

fn delete_dry_run_payload(chat: &str, ids: &[i32]) -> serde_json::Value {
    let would = format!("delete stories {} of {chat}", joined(ids));
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "ids": ids,
        "would": would,
    })
}

fn toggle_dry_run_payload(chat: &str, ids: &[i32], pinned: bool) -> serde_json::Value {
    let action = if pinned { "pin" } else { "unpin" };
    let would = format!("{action} stories {} of {chat}", joined(ids));
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "ids": ids,
        "pinned": pinned,
        "would": would,
    })
}

async fn send(args: SendArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_send_args(&args)?;
    require_explicit_selection("story send", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(send_dry_run_payload(&args));
            }
            validate_upload_path(&args.file)?;
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer = resolve_input_peer(&guard, &args.chat).await?;
            let uploaded = guard
                .client
                .upload_file(&args.file)
                .await
                .map_err(upload_error)?;
            let media = input_media(&args.file, uploaded.raw);
            guard.rate_limiter.acquire().await;
            let resp: tl::enums::Updates = guard
                .client
                .invoke(&tl::functions::stories::SendStory {
                    pinned: args.pinned,
                    noforwards: args.noforwards,
                    fwd_modified: false,
                    peer,
                    media,
                    media_areas: None,
                    caption: args.caption.clone(),
                    entities: None,
                    privacy_rules: privacy_rules(&args.privacy),
                    random_id: story_random_id(),
                    period: args.period,
                    fwd_from_id: None,
                    fwd_from_story: None,
                    albums: None,
                    music: None,
                })
                .await
                .map_err(tele_invocation)?;
            let story_ids = sent_story_ids(&resp);
            if !output::machine_mode(json, jsonl) {
                let line = format!("sent story {} to {}", args.file, args.chat);
                let line = if multi {
                    format!("{name}: {line}")
                } else {
                    line
                };
                output::print_line(&line)?;
            }
            Ok(serde_json::json!({
                "sent": true,
                "chat": args.chat,
                "file": args.file,
                "story_ids": story_ids,
                "privacy": args.privacy,
                "pinned": args.pinned,
                "noforwards": args.noforwards,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::require_chat_target(&args.chat, "chat")?;
    crate::commands::validate_limit(args.limit, 100, "limit")?;
    let mode = if args.archive {
        "archive"
    } else if args.pinned {
        "pinned"
    } else {
        "active"
    };
    let chat_target = args.chat.trim().to_string();
    let limit = args.limit;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = chat_target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(list_dry_run_payload(&chat_target, mode, limit));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer = resolve_input_peer(&guard, &chat_target).await?;
            let (rows, count, pinned_to_top, max_read_id): (
                Vec<serde_json::Value>,
                i32,
                Vec<i32>,
                Option<i32>,
            ) = match mode {
                "active" => {
                    let resp: tl::enums::stories::PeerStories = guard
                        .client
                        .invoke(&tl::functions::stories::GetPeerStories { peer })
                        .await
                        .map_err(tele_invocation)?;
                    let tl::enums::stories::PeerStories::Stories(wrapped) = resp;
                    let tl::enums::PeerStories::Stories(inner) = wrapped.stories;
                    let rows = inner.stories.iter().map(story_row).collect::<Vec<_>>();
                    let count = inner.stories.len() as i32;
                    (rows, count, Vec::new(), inner.max_read_id)
                }
                other => {
                    let resp: tl::enums::stories::Stories = if other == "archive" {
                        guard
                            .client
                            .invoke(&tl::functions::stories::GetStoriesArchive {
                                peer,
                                offset_id: 0,
                                limit: limit as i32,
                            })
                            .await
                            .map_err(tele_invocation)?
                    } else {
                        guard
                            .client
                            .invoke(&tl::functions::stories::GetPinnedStories {
                                peer,
                                offset_id: 0,
                                limit: limit as i32,
                            })
                            .await
                            .map_err(tele_invocation)?
                    };
                    let tl::enums::stories::Stories::Stories(s) = resp;
                    let rows = s.stories.iter().map(story_row).collect::<Vec<_>>();
                    (rows, s.count, s.pinned_to_top.unwrap_or_default(), None)
                }
            };
            let mut rows = rows;
            rows.truncate(limit as usize);
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows.iter().map(table_row).collect();
                output::print_account_table(&name, multi, &STORY_COLUMNS, &table_rows)?;
            }
            Ok(serde_json::json!({
                "chat": chat_target,
                "mode": mode,
                "count": count,
                "stories": rows,
                "pinned_to_top": pinned_to_top,
                "max_read_id": max_read_id,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn read(args: ReadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::require_chat_target(&args.chat, "chat")?;
    if args.max_id <= 0 {
        return Err(TeleError::Usage(
            "--max-id must be a positive story id".to_string(),
        ));
    }
    require_explicit_selection("story read", flags)?;
    let chat_target = args.chat.trim().to_string();
    let max_id = args.max_id;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = chat_target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(read_dry_run_payload(&chat_target, max_id));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer = resolve_input_peer(&guard, &chat_target).await?;
            guard.rate_limiter.acquire().await;
            let returned: Vec<i32> = guard
                .client
                .invoke(&tl::functions::stories::ReadStories { peer, max_id })
                .await
                .map_err(tele_invocation)?;
            if !output::machine_mode(json, jsonl) {
                let line = format!("marked stories up to {max_id} as read for {chat_target}");
                let line = if multi {
                    format!("{name}: {line}")
                } else {
                    line
                };
                output::print_line(&line)?;
            }
            Ok(serde_json::json!({
                "read_max_id": max_id,
                "chat": chat_target,
                "returned_ids": returned,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn delete(args: DeleteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::require_chat_target(&args.chat, "chat")?;
    let ids = parse_ids(&args.ids)?;
    require_explicit_selection("story delete", flags)?;
    let chat_target = args.chat.trim().to_string();
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = chat_target.clone();
        let ids = ids.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(delete_dry_run_payload(&chat_target, &ids));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer = resolve_input_peer(&guard, &chat_target).await?;
            guard.rate_limiter.acquire().await;
            let deleted: Vec<i32> = guard
                .client
                .invoke(&tl::functions::stories::DeleteStories {
                    peer,
                    id: ids.clone(),
                })
                .await
                .map_err(tele_invocation)?;
            if !output::machine_mode(json, jsonl) {
                let line = format!("deleted stories {} from {chat_target}", joined(&deleted));
                let line = if multi {
                    format!("{name}: {line}")
                } else {
                    line
                };
                output::print_line(&line)?;
            }
            Ok(serde_json::json!({
                "chat": chat_target,
                "requested_ids": ids,
                "deleted_ids": deleted,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn toggle_pinned(args: PinArgs, pinned: bool, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::require_chat_target(&args.chat, "chat")?;
    let ids = parse_ids(&args.ids)?;
    let command_label = if pinned { "story pin" } else { "story unpin" };
    require_explicit_selection(command_label, flags)?;
    let chat_target = args.chat.trim().to_string();
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let chat_target = chat_target.clone();
        let ids = ids.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(toggle_dry_run_payload(&chat_target, &ids, pinned));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let peer = resolve_input_peer(&guard, &chat_target).await?;
            guard.rate_limiter.acquire().await;
            let updated: Vec<i32> = guard
                .client
                .invoke(&tl::functions::stories::TogglePinned {
                    peer,
                    id: ids.clone(),
                    pinned,
                })
                .await
                .map_err(tele_invocation)?;
            if !output::machine_mode(json, jsonl) {
                let action = if pinned { "pinned" } else { "unpinned" };
                let line = format!("{action} stories {} on {chat_target}", joined(&updated));
                let line = if multi {
                    format!("{name}: {line}")
                } else {
                    line
                };
                output::print_line(&line)?;
            }
            Ok(serde_json::json!({
                "chat": chat_target,
                "pinned": pinned,
                "requested_ids": ids,
                "updated_ids": updated,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) fn stories_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_USAGE;

    fn input_file_big(name: &str) -> tl::enums::InputFile {
        tl::enums::InputFile::Big(tl::types::InputFileBig {
            id: 9,
            parts: 3,
            name: name.to_string(),
        })
    }

    fn full_item(views: Option<tl::enums::StoryViews>) -> tl::enums::StoryItem {
        tl::enums::StoryItem::Item(Box::new(tl::types::StoryItem {
            pinned: true,
            public: false,
            close_friends: false,
            min: false,
            noforwards: true,
            edited: false,
            contacts: false,
            selected_contacts: false,
            out: true,
            id: 42,
            date: 1_760_000_000,
            from_id: None,
            fwd_from: None,
            expire_date: 1_760_086_400,
            caption: Some("hello".to_string()),
            entities: None,
            media: tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
                spoiler: false,
                live_photo: false,
                photo: None,
                ttl_seconds: None,
                video: None,
            }),
            media_areas: None,
            privacy: None,
            views,
            sent_reaction: None,
            albums: None,
            music: None,
        }))
    }

    fn fake_views(count: i32) -> Option<tl::enums::StoryViews> {
        Some(tl::enums::StoryViews::Views(tl::types::StoryViews {
            has_viewers: false,
            views_count: count,
            forwards_count: Some(2),
            reactions: None,
            reactions_count: Some(3),
            recent_viewers: None,
        }))
    }

    fn sample_send_args() -> SendArgs {
        SendArgs {
            chat: "@someone".to_string(),
            file: "C:/tmp/pic.png".to_string(),
            caption: None,
            privacy: "contacts".to_string(),
            pinned: false,
            noforwards: false,
            period: None,
        }
    }

    #[test]
    fn parse_ids_accepts_positive_lists() {
        assert_eq!(parse_ids("1").unwrap(), vec![1]);
        assert_eq!(parse_ids("1,2,3").unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_ids(" 4 , 5 ").unwrap(), vec![4, 5]);
        assert_eq!(
            parse_ids(i32::MAX.to_string().as_str()).unwrap(),
            vec![i32::MAX]
        );
    }

    #[test]
    fn parse_ids_rejects_garbage_and_non_positive_values() {
        for bad in [
            "",
            "   ",
            ",",
            "a,b",
            "1,,2",
            "0",
            "-1",
            "1.5",
            "99999999999",
            "2147483648",
        ] {
            let err = parse_ids(bad).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad:?}");
            assert_eq!(err.exit_code(), EXIT_USAGE, "for {bad:?}");
            assert!(err.message().contains("--ids"), "for {bad:?}");
        }
    }

    #[test]
    fn validate_privacy_accepts_known_values_only() {
        for good in ["everyone", "contacts", "close-friends"] {
            assert!(validate_privacy(good).is_ok(), "for {good}");
        }
        for bad in ["", "public", "Everyone", "friends"] {
            let err = validate_privacy(bad).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad:?}");
            assert!(err.message().contains("--privacy"), "for {bad:?}");
        }
    }

    #[test]
    fn privacy_rules_map_each_value_to_the_matching_rule() {
        assert!(matches!(
            privacy_rules("everyone")[0],
            tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll
        ));
        assert!(matches!(
            privacy_rules("contacts")[0],
            tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts
        ));
        assert!(matches!(
            privacy_rules("close-friends")[0],
            tl::enums::InputPrivacyRule::InputPrivacyValueAllowCloseFriends
        ));
    }

    #[test]
    fn validate_period_accepts_documented_windows_only() {
        assert!(validate_period(None).is_ok());
        for good in [21_600, 43_200, 86_400, 172_800] {
            assert!(validate_period(Some(good)).is_ok(), "for {good}");
        }
        for bad in [0, -1, 3600, 200_000] {
            let err = validate_period(Some(bad)).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad}");
            assert!(err.message().contains("--period"), "for {bad}");
        }
    }

    #[test]
    fn validate_send_args_matrix() {
        let base = sample_send_args();
        assert!(validate_send_args(&base).is_ok());

        let err = validate_send_args(&SendArgs {
            chat: "  ".to_string(),
            ..base.clone()
        })
        .unwrap_err();
        assert!(err.message().contains("--chat"));

        let err = validate_send_args(&SendArgs {
            file: " ".to_string(),
            ..base.clone()
        })
        .unwrap_err();
        assert!(err.message().contains("--file"));

        let err = validate_send_args(&SendArgs {
            caption: Some("  ".to_string()),
            ..base.clone()
        })
        .unwrap_err();
        assert!(err.message().contains("--caption"));

        let err = validate_send_args(&SendArgs {
            privacy: "public".to_string(),
            ..base
        })
        .unwrap_err();
        assert!(err.message().contains("--privacy"));
    }

    #[test]
    fn mime_type_maps_common_extensions() {
        assert_eq!(mime_type_for("a.JPG"), "image/jpeg");
        assert_eq!(mime_type_for("/x/y/photo.png"), "image/png");
        assert_eq!(mime_type_for("clip.MP4"), "video/mp4");
        assert_eq!(mime_type_for("voice.ogg"), "audio/ogg");
        assert_eq!(mime_type_for("blob.bin"), "application/octet-stream");
        assert_eq!(mime_type_for("noext"), "application/octet-stream");
    }

    #[test]
    fn input_media_wraps_photos_as_uploaded_photo_and_rest_as_document() {
        let photo = input_media("pic.png", input_file_big("pic.png"));
        match photo {
            tl::enums::InputMedia::UploadedPhoto(m) => {
                assert!(matches!(m.file, tl::enums::InputFile::Big(_)));
                assert!(!m.spoiler);
            }
            other => panic!("expected UploadedPhoto, got {other:?}"),
        }
        let doc = input_media("clip.mp4", input_file_big("clip.mp4"));
        match doc {
            tl::enums::InputMedia::UploadedDocument(m) => {
                assert_eq!(m.mime_type, "video/mp4");
                assert!(m.attributes.is_empty());
                assert!(m.thumb.is_none());
            }
            other => panic!("expected UploadedDocument, got {other:?}"),
        }
    }

    #[test]
    fn looks_like_image_covers_story_photo_extensions() {
        for yes in ["a.jpg", "b.JPEG", "c.png", "d.webp", "e.heic"] {
            assert!(looks_like_image(yes), "for {yes}");
        }
        for no in ["clip.mp4", "notes.txt", "noext"] {
            assert!(!looks_like_image(no), "for {no}");
        }
    }

    #[test]
    fn story_random_id_is_positive_and_monotonic() {
        let a = story_random_id();
        let b = story_random_id();
        assert!(a > 0);
        assert!(b >= a);
    }

    #[test]
    fn story_row_shapes_full_items_with_views_and_flags() {
        let row = story_row(&full_item(fake_views(12)));
        assert_eq!(row["id"], serde_json::json!(42));
        assert_eq!(row["date"], serde_json::json!(1_760_000_000));
        assert_eq!(row["expire_date"], serde_json::json!(1_760_086_400));
        assert_eq!(row["caption"], serde_json::json!("hello"));
        assert_eq!(row["pinned"], serde_json::json!(true));
        assert_eq!(row["noforwards"], serde_json::json!(true));
        assert_eq!(row["out"], serde_json::json!(true));
        assert_eq!(row["media_kind"], serde_json::json!("photo"));
        assert_eq!(row["views"]["views_count"], serde_json::json!(12));
        assert_eq!(row["views"]["forwards_count"], serde_json::json!(2));
        assert_eq!(row["views"]["reactions_count"], serde_json::json!(3));

        let plain = story_row(&full_item(None));
        assert!(plain.get("views").is_none());
        assert_eq!(plain["caption"], serde_json::json!("hello"));
    }

    #[test]
    fn story_row_shapes_skipped_and_deleted_variants() {
        let skipped = story_row(&tl::enums::StoryItem::Skipped(
            tl::types::StoryItemSkipped {
                close_friends: true,
                live: false,
                id: 7,
                date: 100,
                expire_date: 200,
            },
        ));
        assert_eq!(skipped["id"], serde_json::json!(7));
        assert_eq!(skipped["skipped"], serde_json::json!(true));
        assert_eq!(skipped["close_friends"], serde_json::json!(true));
        assert_eq!(skipped["live"], serde_json::json!(false));

        let deleted = story_row(&tl::enums::StoryItem::Deleted(
            tl::types::StoryItemDeleted { id: 9 },
        ));
        assert_eq!(deleted["id"], serde_json::json!(9));
        assert_eq!(deleted["deleted"], serde_json::json!(true));
        assert!(deleted.get("date").is_none());
    }

    #[test]
    fn media_kind_raw_names_common_media_variants() {
        let photo = tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
            spoiler: false,
            live_photo: false,
            photo: None,
            ttl_seconds: None,
            video: None,
        });
        assert_eq!(media_kind_raw(&photo), "photo");

        let empty = tl::enums::MessageMedia::Empty;
        assert_eq!(media_kind_raw(&empty), "empty");

        let story_ref = tl::enums::MessageMedia::Story(Box::new(tl::types::MessageMediaStory {
            via_mention: false,
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 5 }),
            id: 1,
            story: None,
        }));
        assert_eq!(media_kind_raw(&story_ref), "story");
    }

    #[test]
    fn table_row_matches_columns_and_state_detection() {
        let active = table_row(&story_row(&full_item(fake_views(12))));
        assert_eq!(active.len(), STORY_COLUMNS.len());
        assert_eq!(STORY_COLUMNS[0], "id");
        assert_eq!(active[0], "42");
        assert_eq!(active[1], "active");
        assert_eq!(active[4], "hello");
        assert_eq!(active[5], "true");
        assert_eq!(active[6], "12");

        let deleted_row = table_row(&story_row(&tl::enums::StoryItem::Deleted(
            tl::types::StoryItemDeleted { id: 9 },
        )));
        assert_eq!(deleted_row[1], "deleted");
        assert_eq!(deleted_row[6], "-");

        let skipped = table_row(&story_row(&tl::enums::StoryItem::Skipped(
            tl::types::StoryItemSkipped {
                close_friends: false,
                live: true,
                id: 3,
                date: 1,
                expire_date: 2,
            },
        )));
        assert_eq!(skipped[1], "skipped");
    }

    #[test]
    fn truncate_caption_shortens_long_text_without_panic_on_multibyte() {
        assert_eq!(truncate_caption("short", 40), "short");
        let long = "a".repeat(50);
        let cut = truncate_caption(&long, 40);
        assert!(cut.starts_with(&"a".repeat(40)));
        assert!(cut.ends_with('…'));
        assert_eq!(cut.chars().count(), 41);
        assert_eq!(truncate_caption("приветмир", 40), "приветмир");
    }

    #[test]
    fn sent_story_ids_extracts_update_story_entries() {
        let updates = tl::enums::Updates::Updates(tl::types::Updates {
            updates: vec![
                tl::enums::Update::Story(tl::types::UpdateStory {
                    peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 5 }),
                    story: full_item(None),
                }),
                tl::enums::Update::User(tl::types::UpdateUser { user_id: 11 }),
            ],
            users: Vec::new(),
            chats: Vec::new(),
            date: 0,
            seq: 0,
        });
        assert_eq!(sent_story_ids(&updates), vec![42]);

        let short = tl::enums::Updates::TooLong;
        assert!(sent_story_ids(&short).is_empty());
    }

    #[test]
    fn dry_run_payloads_carry_would_text_and_arguments() {
        let args = SendArgs {
            caption: Some("cap".to_string()),
            privacy: "close-friends".to_string(),
            pinned: true,
            period: Some(86_400),
            ..sample_send_args()
        };
        let payload = send_dry_run_payload(&args);
        assert_eq!(payload["dry_run"], serde_json::json!(true));
        assert_eq!(
            payload["would"],
            serde_json::json!("send story C:/tmp/pic.png to @someone")
        );
        assert_eq!(payload["privacy"], serde_json::json!("close-friends"));
        assert_eq!(payload["pinned"], serde_json::json!(true));
        assert_eq!(payload["period"], serde_json::json!(86_400));

        let payload = list_dry_run_payload("@someone", "archive", 25);
        assert_eq!(payload["mode"], serde_json::json!("archive"));
        assert_eq!(
            payload["would"],
            serde_json::json!("list archive stories of @someone")
        );

        let payload = read_dry_run_payload("@someone", 33);
        assert_eq!(payload["max_id"], serde_json::json!(33));
        assert_eq!(
            payload["would"],
            serde_json::json!("mark stories up to 33 as read for @someone")
        );

        let payload = delete_dry_run_payload("@someone", &[1, 2]);
        assert_eq!(payload["ids"], serde_json::json!([1, 2]));
        assert_eq!(
            payload["would"],
            serde_json::json!("delete stories 1,2 of @someone")
        );

        let payload = toggle_dry_run_payload("@someone", &[4], true);
        assert_eq!(payload["pinned"], serde_json::json!(true));
        assert_eq!(
            payload["would"],
            serde_json::json!("pin stories 4 of @someone")
        );
        let payload = toggle_dry_run_payload("@someone", &[4], false);
        assert_eq!(
            payload["would"],
            serde_json::json!("unpin stories 4 of @someone")
        );

        for payload in [
            send_dry_run_payload(&args),
            list_dry_run_payload("me", "active", 10),
            read_dry_run_payload("me", 1),
            delete_dry_run_payload("me", &[1]),
            toggle_dry_run_payload("me", &[1], true),
        ] {
            assert_eq!(payload["dry_run"], serde_json::json!(true));
        }
    }

    #[test]
    fn joined_formats_comma_separated_ids() {
        assert_eq!(joined(&[]), "");
        assert_eq!(joined(&[1]), "1");
        assert_eq!(joined(&[1, 2, 30]), "1,2,30");
    }

    #[test]
    fn story_columns_are_stable() {
        assert_eq!(STORY_COLUMNS.len(), 7);
        assert_eq!(STORY_COLUMNS[0], "id");
        assert_eq!(STORY_COLUMNS[1], "state");
        assert_eq!(STORY_COLUMNS[6], "views");
    }
}
