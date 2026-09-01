use grammers_client::message::InputMessage;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::helpers::upload_error;
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

use super::params::{SendArgs, SendParams};
use crate::commands::helpers::looks_like_image;

fn effective_preview(args: &SendArgs) -> bool {
    args.preview && !args.no_preview
}

pub(crate) fn validate_send(args: &SendArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    if let Some(topic) = args.topic {
        if topic <= 0 {
            return Err(TeleError::Usage(
                "--topic must be a positive topic ID".to_string(),
            ));
        }
    }
    match args.format.as_str() {
        "plain" | "markdown" => Ok(()),
        other => Err(TeleError::Usage(format!(
            "unknown --format {other} (use plain or markdown)"
        ))),
    }?;
    match (
        &args.text,
        args.files.is_empty(),
        &args.url,
        &args.copy_from,
    ) {
        (None, true, None, None) => {
            return Err(TeleError::Usage(
                "msg send requires --text, --file, --url, or --copy-from".to_string(),
            ))
        }
        (Some(_), false, _, _) | (Some(_), _, Some(_), _) => {
            return Err(TeleError::Usage(
                "--text is mutually exclusive with --file/--url/--copy-from".to_string(),
            ))
        }
        (_, false, Some(_), _) => {
            return Err(TeleError::Usage(
                "--file and --url are mutually exclusive".to_string(),
            ))
        }
        _ => {}
    }
    if args.copy_from.is_some() != args.copy_id.is_some() {
        return Err(TeleError::Usage(
            "--copy-from requires --copy-id and vice versa".to_string(),
        ));
    }
    if args.copy_from.is_some()
        && (args.text.is_some() || !args.files.is_empty() || args.url.is_some())
    {
        return Err(TeleError::Usage(
            "--copy-from is mutually exclusive with --text/--file/--url".to_string(),
        ));
    }
    if !args.files.is_empty() && args.files.len() > 10 {
        return Err(TeleError::Usage(
            "albums support at most 10 files; send larger sets in batches".to_string(),
        ));
    }
    if let Some(ttl) = args.media_ttl {
        if ttl <= 0 {
            return Err(TeleError::Usage("--media-ttl must be positive".to_string()));
        }
    }
    if let Some(text) = &args.text {
        if text.trim().is_empty() {
            return Err(TeleError::Usage("--text must not be empty".to_string()));
        }
    }
    if let Some(kind) = &args.kind {
        if kind != "photo" && kind != "document" {
            return Err(TeleError::Usage(
                "--kind must be photo or document".to_string(),
            ));
        }
    }
    if args.url.is_some() && args.kind.is_none() {
        return Err(TeleError::Usage(
            "--url requires --kind photo|document".to_string(),
        ));
    }
    if args.caption.as_ref().is_some_and(|c| c.trim().is_empty()) {
        return Err(TeleError::Usage("--caption must not be empty".to_string()));
    }
    if args.caption.is_some()
        && args.files.is_empty()
        && args.copy_from.is_none()
        && args.url.is_none()
    {
        return Err(TeleError::Usage("--caption requires --file".to_string()));
    }
    if args.noforwards && (!args.files.is_empty() || args.url.is_some() || args.copy_from.is_some())
    {
        return Err(TeleError::Usage(
            "--noforwards supports text sends only at this layer".to_string(),
        ));
    }
    if args.background && args.files.len() > 1 {
        return Err(TeleError::Usage(
            "--background is not supported with albums".to_string(),
        ));
    }
    if args.silent && args.files.len() > 1 {
        return Err(TeleError::Usage(
            "--silent is not supported with albums".to_string(),
        ));
    }
    if args.media_ttl.is_some() && args.files.len() > 1 {
        return Err(TeleError::Usage(
            "--media-ttl is not supported with albums".to_string(),
        ));
    }
    if !args.files.is_empty() && !effective_preview(args) {
        return Err(TeleError::Usage(
            "--no-preview is not supported with --file".to_string(),
        ));
    }
    if let Some(thumb) = &args.thumbnail {
        super::validate::validate_upload_path(thumb)?;
        if args.files.len() > 1 {
            return Err(TeleError::Usage(
                "--thumbnail applies to a single document upload".to_string(),
            ));
        }
    }
    if args.schedule.as_deref() == Some("online") && !args.files.is_empty() {
        return Err(TeleError::Usage(
            "--schedule online is not supported with albums".to_string(),
        ));
    }
    if args.schedule.as_deref() == Some("online")
        && (args.url.is_some() || args.copy_from.is_some())
    {
        return Err(TeleError::Usage(
            "--schedule is not supported with --url or --copy-from".to_string(),
        ));
    }
    if args
        .schedule
        .as_deref()
        .is_some_and(|s| !s.eq_ignore_ascii_case("online"))
        && (args.files.len() > 1 || args.url.is_some() || args.copy_from.is_some())
    {
        return Err(TeleError::Usage(
            "--schedule is not supported with --url, --copy-from, or albums (2+ files)".to_string(),
        ));
    }
    if args.format == "markdown" {
        if let Some(text) = &args.text {
            super::validate::validate_markdown(text)?;
        }
        if let Some(caption) = &args.caption {
            super::validate::validate_markdown(caption)?;
        }
    }
    Ok(())
}

const SCHEDULE_ONLINE_SENTINEL: i32 = 0;

pub(crate) fn parse_schedule(value: Option<&str>) -> TeleResult<Option<i32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.eq_ignore_ascii_case("online") {
        return Ok(Some(SCHEDULE_ONLINE_SENTINEL));
    }
    let dt = crate::commands::parse_unixtime(value)?;
    let ts = dt.timestamp();
    if ts <= chrono::Utc::now().timestamp() {
        return Err(TeleError::Usage(format!(
            "--schedule must be a future time (got {value})"
        )));
    }
    i32::try_from(ts).map(Some).map_err(|_| {
        TeleError::Usage(format!(
            "--schedule {value} is out of range (max 2038-01-19 03:14:07 UTC)"
        ))
    })
}

const SCHEDULE_ONCE_ONLINE_RAW: i32 = 0x7fff_fffe;

pub(crate) fn message_random_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST_ID: AtomicI64 = AtomicI64::new(0);
    if LAST_ID.load(Ordering::SeqCst) == 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(1);
        LAST_ID.fetch_max(now.max(1), Ordering::SeqCst);
    }
    LAST_ID.fetch_add(1, Ordering::SeqCst)
}

fn raw_input_reply_to(reply_to_msg_id: i32) -> grammers_client::tl::enums::InputReplyTo {
    grammers_client::tl::types::InputReplyToMessage {
        reply_to_msg_id,
        top_msg_id: None,
        reply_to_peer_id: None,
        quote_text: None,
        quote_entities: None,
        quote_offset: None,
        monoforum_peer_id: None,
        todo_item_id: None,
        poll_option: None,
    }
    .into()
}

fn rfc3339_ts(ts: i32) -> String {
    chrono::DateTime::from_timestamp(i64::from(ts), 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn merge_sent_updates(
    row: &mut serde_json::Map<String, serde_json::Value>,
    updates: &[grammers_client::tl::enums::Update],
) {
    use grammers_client::tl;
    for update in updates {
        match update {
            tl::enums::Update::MessageId(m) => {
                row.entry("id".to_string())
                    .or_insert_with(|| serde_json::json!(m.id));
            }
            tl::enums::Update::NewMessage(n) => {
                if let tl::enums::Message::Message(m) = &n.message {
                    row.insert("id".into(), serde_json::json!(m.id));
                    row.insert("out".into(), serde_json::json!(m.out));
                    row.insert("date".into(), serde_json::json!(rfc3339_ts(m.date)));
                }
            }
            _ => {}
        }
    }
    if !row.contains_key("id") {
        row.insert("sent".into(), serde_json::json!(true));
    }
}

fn sent_updates_row(
    updates: &grammers_client::tl::enums::Updates,
    text: &str,
) -> serde_json::Value {
    use grammers_client::tl;
    let mut row = serde_json::Map::new();
    row.insert("text".into(), serde_json::json!(text));
    row.insert("noforwards".into(), serde_json::json!(true));
    match updates {
        tl::enums::Updates::UpdateShortSentMessage(s) => {
            row.insert("id".into(), serde_json::json!(s.id));
            row.insert("out".into(), serde_json::json!(s.out));
            row.insert("date".into(), serde_json::json!(rfc3339_ts(s.date)));
        }
        tl::enums::Updates::Updates(u) => merge_sent_updates(&mut row, &u.updates),
        tl::enums::Updates::Combined(c) => merge_sent_updates(&mut row, &c.updates),
        _ => {
            row.insert("sent".into(), serde_json::json!(true));
        }
    }
    serde_json::Value::Object(row)
}

struct RawTextSend<'a> {
    text: &'a str,
    format: &'a str,
    preview: bool,
    silent: bool,
    background: bool,
    reply: Option<i32>,
    schedule: Option<u64>,
}

async fn send_noforwards_text(
    client: &grammers_client::Client,
    chat: &grammers_client::peer::Peer,
    peer: grammers_client::tl::enums::InputPeer,
    spec: RawTextSend<'_>,
) -> TeleResult<serde_json::Value> {
    use grammers_client::parsers::parse_markdown_message;
    use grammers_client::tl;
    let RawTextSend {
        text,
        format,
        preview,
        silent,
        background,
        reply,
        schedule,
    } = spec;
    let (message, entities) = match format {
        "markdown" => parse_markdown_message(text),
        _ => (text.to_string(), Vec::new()),
    };
    let schedule_date = schedule.map(|s| {
        if s == 0 {
            SCHEDULE_ONCE_ONLINE_RAW
        } else {
            s as i32
        }
    });
    let updates: tl::enums::Updates = client
        .invoke(&tl::functions::messages::SendMessage {
            no_webpage: !preview,
            silent,
            background,
            clear_draft: false,
            peer,
            reply_to: reply.map(raw_input_reply_to),
            message: message.clone(),
            random_id: message_random_id(),
            reply_markup: None,
            entities: if entities.is_empty() {
                None
            } else {
                Some(entities)
            },
            schedule_date,
            schedule_repeat_period: None,
            send_as: None,
            noforwards: true,
            update_stickersets_order: false,
            invert_media: false,
            quick_reply_shortcut: None,
            effect: None,
            allow_paid_floodskip: false,
            allow_paid_stars: None,
            suggested_post: None,
            rich_message: None,
        })
        .await
        .map_err(tele_invocation)?;
    let mut row = sent_updates_row(&updates, &message);
    if let Some(obj) = row.as_object_mut() {
        obj.insert("peer".into(), crate::serialize::peer_key(chat));
    }
    Ok(row)
}

pub(crate) fn send_dry_run_payload(args: &SendArgs, schedule: Option<u64>) -> serde_json::Value {
    let would = format!("send message to chat {}", args.chat);
    serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "text": args.text,
        "files": args.files,
        "url": args.url,
        "kind": args.kind,
        "copy_from": args.copy_from,
        "copy_id": args.copy_id,
        "media_ttl": args.media_ttl,
        "thumbnail": args.thumbnail,
        "caption": args.caption,
        "format": args.format,
        "schedule": schedule,
        "reply": args.reply,
        "topic": args.topic,
        "preview": effective_preview(args),
        "silent": args.silent,
        "noforwards": args.noforwards,
        "background": args.background,
        "would": would})
}

pub(crate) fn send_serve_dry_run(args: &SendArgs) -> TeleResult<serde_json::Value> {
    let schedule = parse_schedule(args.schedule.as_deref())?.map(|s| s as u64);
    Ok(send_dry_run_payload(args, schedule))
}

pub(crate) async fn send_core(
    shares: &crate::client::ServeShares,
    params: SendParams,
) -> TeleResult<serde_json::Value> {
    for path in &params.files {
        super::validate::validate_upload_path(path)?;
    }
    if let Some(thumb) = &params.thumbnail {
        super::validate::validate_upload_path(thumb)?;
    }
    shares.rate_limiter.acquire().await;
    let chat_target = params.chat.clone();
    let text = params.text.clone();
    let caption = params.caption.clone();
    let files = params.files.clone();
    let format = params.format.clone();
    let schedule = parse_schedule(params.schedule.as_deref())?.map(|s| s as u64);
    let reply = params.reply.or(params.topic);
    let preview = params.preview && !params.no_preview;
    let silent = params.silent;
    let noforwards = params.noforwards;
    let background = params.background;
    let media_ttl = params.media_ttl;
    let thumbnail = params.thumbnail.clone();
    let url = params.url.clone();
    let kind = params.kind.clone();
    let copy_from = params.copy_from.clone();
    let copy_id = params.copy_id;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &chat_target).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let apply_common = |msg: InputMessage| -> InputMessage {
        let mut msg = msg.reply_to(reply);
        if silent {
            msg = msg.silent(true);
        }
        if background {
            msg = msg.background(true);
        }
        if let Some(ttl) = media_ttl {
            msg = msg.media_ttl(ttl);
        }
        msg
    };
    if let Some(src_chat) = &copy_from {
        let id = copy_id
            .ok_or_else(|| TeleError::Usage("copy-id required with copy-from".to_string()))?;
        let src = entities::resolve_peer(&shares.client, shares.session.as_ref(), src_chat).await?;
        let src_ref = entities::peer_ref(&src).await.map_err(tele_invocation)?;
        let found = shares
            .client
            .get_messages_by_id(src_ref, &[id])
            .await
            .map_err(tele_invocation)?;
        let source_msg = found
            .into_iter()
            .flatten()
            .next()
            .ok_or_else(|| TeleError::Invocation(format!("message {id} not found"), None))?;
        let media = source_msg.media().ok_or_else(|| {
            TeleError::Invocation(format!("message {id} has no media to copy"), None)
        })?;
        let base = match format.as_str() {
            "markdown" => InputMessage::new()
                .copy_media(&media)
                .markdown(caption.clone().unwrap_or_default()),
            _ => InputMessage::new()
                .copy_media(&media)
                .text(caption.clone().unwrap_or_default()),
        };
        let base = base.link_preview(preview);
        let sent = shares
            .client
            .send_message(chat_ref, apply_common(base))
            .await
            .map_err(tele_invocation)?;
        let mut row = crate::serialize::message_to_json(&sent)?;
        crate::serialize::upgrade_peer_identity(&mut row, &chat);
        return Ok(row);
    }
    if let Some(link) = &url {
        let base = match kind.as_deref() {
            Some("document") => InputMessage::new().document_url(link),
            _ => InputMessage::new().photo_url(link),
        };
        let base = if let Some(cap) = &caption {
            match format.as_str() {
                "markdown" => base.markdown(cap.clone()),
                _ => base.text(cap.clone()),
            }
        } else {
            base
        };
        let base = base.link_preview(preview);
        let sent = shares
            .client
            .send_message(chat_ref, apply_common(base))
            .await
            .map_err(tele_invocation)?;
        let mut row = crate::serialize::message_to_json(&sent)?;
        crate::serialize::upgrade_peer_identity(&mut row, &chat);
        return Ok(row);
    }
    if files.len() > 1 {
        let mut medias: Vec<grammers_client::media::InputMedia> = Vec::new();
        for (idx, path) in files.iter().enumerate() {
            let uploaded = shares
                .client
                .upload_file(path)
                .await
                .map_err(upload_error)?;
            let mut media = match looks_like_image(path) {
                true => grammers_client::media::InputMedia::new().photo(uploaded),
                false => grammers_client::media::InputMedia::new().document(uploaded),
            };
            let cap = if idx == 0 {
                caption.clone().unwrap_or_default()
            } else {
                String::new()
            };
            media = match format.as_str() {
                "markdown" => media.markdown(cap),
                _ => media.caption(cap),
            };
            media = media.reply_to(reply);
            medias.push(media);
        }
        let sent_album = shares
            .client
            .send_album(chat_ref, medias)
            .await
            .map_err(tele_invocation)?;
        let mut rows = Vec::new();
        for m in sent_album.into_iter().flatten() {
            rows.push(crate::serialize::message_to_json(&m)?);
        }
        return Ok(serde_json::json!({"album": rows}));
    }
    let mut msg = if let Some(path) = files.first() {
        let uploaded = shares
            .client
            .upload_file(path)
            .await
            .map_err(upload_error)?;
        let mut base = match format.as_str() {
            "markdown" => InputMessage::new().markdown(caption.unwrap_or_default()),
            _ => InputMessage::new().text(caption.unwrap_or_default()),
        };
        if let Some(thumb_path) = &thumbnail {
            let thumb_uploaded = shares
                .client
                .upload_file(thumb_path)
                .await
                .map_err(upload_error)?;
            base = base.thumbnail(thumb_uploaded);
        }
        if looks_like_image(path) {
            base.photo(uploaded)
        } else {
            base.document(uploaded)
        }
    } else {
        let text = text.as_deref().unwrap_or_default();
        if noforwards {
            let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
            return send_noforwards_text(
                &shares.client,
                &chat,
                peer,
                RawTextSend {
                    text,
                    format: format.as_str(),
                    preview,
                    silent,
                    background,
                    reply,
                    schedule,
                },
            )
            .await;
        }
        let base = match format.as_str() {
            "markdown" => InputMessage::new().markdown(text),
            _ => InputMessage::new().text(text),
        };
        base.link_preview(preview)
    };
    if let Some(s) = schedule {
        if s == 0 {
            msg = msg.schedule_once_online();
        } else {
            let ts = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s);
            msg = msg.schedule_date(Some(ts));
        }
    }
    msg = apply_common(msg);
    let sent = shares
        .client
        .send_message(chat_ref, msg)
        .await
        .map_err(tele_invocation)?;
    let mut row = crate::serialize::message_to_json(&sent)?;
    crate::serialize::upgrade_peer_identity(&mut row, &chat);
    Ok(row)
}

pub(crate) async fn send(args: SendArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_send(&args)?;
    crate::executor::require_explicit_selection("msg send", flags)?;
    for path in &args.files {
        super::validate::validate_upload_path_inner(path, flags.dry_run)?;
    }
    if let Some(thumb) = &args.thumbnail {
        super::validate::validate_upload_path_inner(thumb, flags.dry_run)?;
    }
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let schedule = parse_schedule(args.schedule.as_deref())?.map(|s| s as u64);
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(send_dry_run_payload(&args, schedule));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            send_core(&guard.shares(), SendParams::from(&args)).await
        })
    })
    .await?;
    if dry_run && !output::machine_mode(json, jsonl) {
        if let Some(first) = envelope.accounts.first() {
            if let Some(data) = &first.data {
                if let Some(would) = data.get("would").and_then(|w| w.as_str()) {
                    output::print_line(would)?;
                }
            }
        }
    }
    crate::executor::finish(flags, &envelope)
}
