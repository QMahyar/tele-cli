use clap::Subcommand;
use grammers_client::message::InputMessage;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::helpers::{looks_like_image, upload_error};
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

pub mod download;
pub mod params;
pub mod send;
pub mod validate;

use download::{download, download_core, download_serve_dry_run, validate_download};
use send::{send, send_core, send_serve_dry_run};

pub use params::{
    ClickArgs, DeleteArgs, DownloadArgs, EditArgs, ForwardArgs, GetArgs, PinArgs, ReactArgs,
    ReadArgs, ScheduledArgs, ScheduledDeleteArgs, ScheduledSendArgs, SearchArgs, SendArgs,
    TypingArgs, VoteArgs,
};
pub(crate) use params::{
    ClickParams, DeleteParams, DownloadParams, EditParams, ForwardParams, GetParams, PinParams,
    ReactParams, ReadParams, ScheduledDeleteParams, ScheduledParams, ScheduledSendParams,
    SearchParams, SendParams, TypingParams, VoteParams,
};
pub(crate) use send::validate_send;
pub use validate::validate_upload_path;

#[derive(Subcommand)]
pub enum MsgCmd {
    Send(Box<SendArgs>),
    Edit(EditArgs),
    Delete(DeleteArgs),
    Forward(ForwardArgs),
    Pin(PinArgs),
    Get(GetArgs),
    Read(ReadArgs),
    React(ReactArgs),
    Search(SearchArgs),
    Download(DownloadArgs),
    Vote(VoteArgs),
    Typing(TypingArgs),
    Click(ClickArgs),
    #[command(about = "list scheduled messages for a chat")]
    Scheduled(ScheduledArgs),
    #[command(about = "delete scheduled messages by id")]
    ScheduledDelete(ScheduledDeleteArgs),
    #[command(about = "send scheduled messages now by id")]
    ScheduledSend(ScheduledSendArgs),
}

pub async fn run(cmd: MsgCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        MsgCmd::Send(a) => send(*a, flags).await,
        MsgCmd::Edit(a) => edit(a, flags).await,
        MsgCmd::Delete(a) => delete(a, flags).await,
        MsgCmd::Forward(a) => forward(a, flags).await,
        MsgCmd::Pin(a) => pin(a, flags).await,
        MsgCmd::Get(a) => get(a, flags).await,
        MsgCmd::Read(a) => read(a, flags).await,
        MsgCmd::React(a) => react(a, flags).await,
        MsgCmd::Search(a) => search(a, flags).await,
        MsgCmd::Download(a) => download(a, flags).await,
        MsgCmd::Vote(a) => vote(a, flags).await,
        MsgCmd::Typing(a) => typing(a, flags).await,
        MsgCmd::Click(a) => click(a, flags).await,
        MsgCmd::Scheduled(a) => scheduled(a, flags).await,
        MsgCmd::ScheduledDelete(a) => scheduled_delete(a, flags).await,
        MsgCmd::ScheduledSend(a) => scheduled_send(a, flags).await,
    }
}

pub(crate) fn validate_edit(args: &EditArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    match args.format.as_str() {
        "plain" | "markdown" => Ok(()),
        other => Err(TeleError::Usage(format!(
            "unknown --format {other} (use plain or markdown)"
        ))),
    }?;
    if args.file.is_some() && args.text.is_some() {
        return Err(TeleError::Usage(
            "--file and --text are mutually exclusive".to_string(),
        ));
    }
    if args.file.is_none() {
        match &args.text {
            None => {
                return Err(TeleError::Usage(
                    "msg edit requires --text or --file".to_string(),
                ))
            }
            Some(text) if text.trim().is_empty() => {
                return Err(TeleError::Usage("--text must not be empty".to_string()));
            }
            _ => {}
        }
    }
    if let Some(caption) = &args.caption {
        if args.file.is_none() {
            return Err(TeleError::Usage("--caption requires --file".to_string()));
        }
        if caption.trim().is_empty() {
            return Err(TeleError::Usage("--caption must not be empty".to_string()));
        }
    }
    if args.format == "markdown" {
        if let Some(text) = &args.text {
            validate::validate_markdown(text)?;
        }
        if let Some(caption) = &args.caption {
            validate::validate_markdown(caption)?;
        }
    }
    Ok(())
}

pub(crate) fn edit_dry_run_payload(args: &EditArgs) -> serde_json::Value {
    serde_json::json!({
    "dry_run": true,
    "id": args.id,
    "chat": args.chat.as_str(),
    "text": args.text,
    "file": args.file,
    "caption": args.caption,
    "format": args.format,
    "preview": !args.no_preview,
    "would": if args.file.is_some() {
        format!("edit media of message {}", args.id)
    } else {
        format!("edit message {}", args.id)
    }})
}

pub(crate) fn edit_serve_dry_run(args: &EditArgs) -> TeleResult<serde_json::Value> {
    Ok(edit_dry_run_payload(args))
}

async fn edit(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_edit(&args)?;
    if let Some(path) = &args.file {
        validate::validate_upload_path_inner(path, flags.dry_run)?;
    }
    crate::executor::require_explicit_selection("msg edit", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return edit_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            edit_core(&guard.shares(), EditParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn edit_core(
    shares: &crate::client::ServeShares,
    params: EditParams,
) -> TeleResult<serde_json::Value> {
    match params.format.as_str() {
        "plain" | "markdown" => Ok(()),
        other => Err(TeleError::Usage(format!(
            "unknown --format {other} (use plain or markdown)"
        ))),
    }?;
    if params.file.is_some() && params.text.is_some() {
        return Err(TeleError::Usage(
            "--file and --text are mutually exclusive".to_string(),
        ));
    }
    if let Some(path) = &params.file {
        validate::validate_upload_path(path)?;
    }
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let preview = !params.no_preview;
    let message = if let Some(path) = &params.file {
        let uploaded = shares
            .client
            .upload_file(path)
            .await
            .map_err(upload_error)?;
        let caption = params.caption.clone().unwrap_or_default();
        let base = match params.format.as_str() {
            "markdown" => InputMessage::new().markdown(caption),
            _ => InputMessage::new().text(caption),
        };
        let media = if looks_like_image(path) {
            base.photo(uploaded)
        } else {
            base.document(uploaded)
        };
        media.link_preview(preview)
    } else {
        let text = params.text.clone().unwrap_or_default();
        if text.trim().is_empty() {
            return Err(TeleError::Usage(
                "msg edit requires --text or --file".to_string(),
            ));
        }
        let base = match params.format.as_str() {
            "markdown" => InputMessage::new().markdown(text),
            _ => InputMessage::new().text(text),
        };
        base.link_preview(preview)
    };
    shares
        .client
        .edit_message(chat_ref, params.id, message)
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({"id": params.id, "edited": true}))
}

pub(crate) fn validate_delete(args: &DeleteArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.all && !args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--all and --ids are mutually exclusive".to_string(),
        ));
    }
    if !args.all && args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--ids required unless --all is used".to_string(),
        ));
    }
    if args.all && args.self_only {
        return Err(TeleError::Usage(
            "--all and --self-only are mutually exclusive".to_string(),
        ));
    }
    Ok(())
}

fn self_only_supported(kind: grammers_session::types::PeerKind) -> bool {
    !matches!(kind, grammers_session::types::PeerKind::Channel)
}

fn delete_report(requested: usize, deleted: usize) -> (serde_json::Value, bool) {
    let partial = requested > 0 && deleted < requested;
    let mut value = serde_json::json!({"requested": requested, "deleted": deleted});
    if partial {
        value["partial"] = serde_json::json!(true);
    }
    (value, partial)
}

pub(crate) fn delete_serve_dry_run(args: &DeleteArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
    "dry_run": true,
    "ids": args.ids,
    "self_only": args.self_only,
    "would": if args.all {
        format!("delete all messages in chat {}", args.chat)
    } else {
        format!("delete {} message(s) by id", args.ids.len())
    }}))
}

async fn delete(args: DeleteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_delete(&args)?;
    crate::executor::require_explicit_selection("msg delete", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return delete_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            delete_core(&guard.shares(), DeleteParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn delete_core(
    shares: &crate::client::ServeShares,
    params: DeleteParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let all = params.all;
    let ids = params.ids;
    let self_only = params.self_only;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    if all {
        let mut iter = shares.client.iter_messages(chat_ref);
        let mut count = 0usize;
        let mut requested = 0usize;
        let mut batch: Vec<i32> = Vec::with_capacity(100);
        while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
            requested += 1;
            batch.push(msg.id());
            if batch.len() >= 100 {
                shares
                    .client
                    .delete_messages(chat_ref, &batch)
                    .await
                    .map_err(tele_invocation)?;
                count += batch.len();
                batch.clear();
            }
        }
        if !batch.is_empty() {
            shares
                .client
                .delete_messages(chat_ref, &batch)
                .await
                .map_err(tele_invocation)?;
            count += batch.len();
        }
        let (mut report, partial) = delete_report(requested, count);
        report["unconfirmed"] = serde_json::json!(true);
        if partial {
            crate::output::log_line(
                "warn",
                &format!("delete removed {count} of {requested} requested message(s)"),
            );
        }
        return Ok(report);
    }
    let mut count = 0usize;
    if self_only {
        if !self_only_supported(chat.id().kind()) {
            return Err(TeleError::Usage(
                "--self-only is not supported in channels".to_string(),
            ));
        }
        for chunk in batches(&ids) {
            shares
                .client
                .invoke(&grammers_client::tl::functions::messages::DeleteMessages {
                    revoke: false,
                    id: chunk.to_vec(),
                })
                .await
                .map_err(tele_invocation)?;
            count += chunk.len();
        }
    } else {
        for chunk in batches(&ids) {
            shares
                .client
                .delete_messages(chat_ref, chunk)
                .await
                .map_err(tele_invocation)?;
            count += chunk.len();
        }
    }
    let (report, partial) = delete_report(ids.len(), count);
    if partial {
        crate::output::log_line(
            "warn",
            &format!("delete removed {count} of {} requested id(s)", ids.len()),
        );
    }
    Ok(report)
}

pub(crate) fn validate_forward(args: &ForwardArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.from.as_str(), "from")?;
    crate::chat_target::ChatTarget::parse_flag(args.to.as_str(), "to")?;
    if args.ids.is_empty() {
        return Err(TeleError::Usage("--ids required".to_string()));
    }
    Ok(())
}

pub(crate) fn forward_serve_dry_run(args: &ForwardArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "ids": args.ids,
        "would": format!("forward {} message(s) to chat {}", args.ids.len(), args.to)
    }))
}

async fn forward(args: ForwardArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_forward(&args)?;
    crate::executor::require_explicit_selection("msg forward", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return forward_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            forward_core(&guard.shares(), ForwardParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn forward_core(
    shares: &crate::client::ServeShares,
    params: ForwardParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let ids = params.ids;
    let from =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.from).await?;
    let to = entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.to).await?;
    let from_ref = entities::peer_ref(&from).await.map_err(tele_invocation)?;
    let to_ref = entities::peer_ref(&to).await.map_err(tele_invocation)?;
    let mut forwarded: Vec<serde_json::Value> = Vec::new();
    let mut dropped: Vec<i32> = Vec::new();
    let mut failed: Vec<i32> = Vec::new();
    for chunk in batches(&ids) {
        let sent = shares
            .client
            .forward_messages(to_ref, chunk, from_ref)
            .await
            .map_err(tele_invocation);
        match sent {
            Ok(results) => push_forward_results(&mut forwarded, &mut dropped, chunk, results)?,
            Err(e) => {
                crate::output::log_line(
                    "warn",
                    &format!("forward failed for {} id(s): {e}", chunk.len()),
                );
                failed.extend_from_slice(chunk);
            }
        }
    }
    let (report, should_warn) = forward_report(ids.len(), &forwarded, &dropped, &failed);
    if should_warn {
        crate::output::log_line(
            "warn",
            &format!("forward confirmed 0 of {} requested id(s)", ids.len()),
        );
    }
    Ok(report)
}

fn batches(ids: &[i32]) -> Vec<&[i32]> {
    ids.chunks(100).collect()
}

fn push_forward_results(
    forwarded: &mut Vec<serde_json::Value>,
    dropped: &mut Vec<i32>,
    chunk: &[i32],
    results: Vec<Option<grammers_client::message::Message>>,
) -> TeleResult<()> {
    for (i, m) in results.into_iter().enumerate() {
        match m {
            Some(m) => forwarded.push(crate::serialize::message_to_json(&m)?),
            None => {
                if let Some(id) = chunk.get(i) {
                    dropped.push(*id);
                }
            }
        }
    }
    Ok(())
}

fn forward_report(
    requested: usize,
    forwarded: &[serde_json::Value],
    dropped: &[i32],
    failed: &[i32],
) -> (serde_json::Value, bool) {
    let mut value = serde_json::json!({"requested": requested, "forwarded": forwarded});
    if !dropped.is_empty() {
        value["dropped"] = serde_json::json!({"count": dropped.len(), "ids": dropped});
    }
    if !failed.is_empty() {
        value["failed"] = serde_json::json!({"count": failed.len(), "ids": failed});
    }
    let partial = requested > 0 && forwarded.len() < requested;
    if partial {
        value["partial"] = serde_json::json!(true);
    }
    let should_warn = requested > 0 && forwarded.is_empty();
    (value, should_warn)
}

pub(crate) fn pin_serve_dry_run(args: &PinArgs) -> TeleResult<serde_json::Value> {
    let would = if args.show {
        "show pinned message".to_string()
    } else if args.all {
        "unpin all messages".to_string()
    } else {
        format!(
            "{} message {}",
            if args.unpin { "unpin" } else { "pin" },
            args.id.unwrap_or_default()
        )
    };
    Ok(serde_json::json!({
        "dry_run": true,
        "id": args.id,
        "unpin": args.unpin,
        "show": args.show,
        "all": args.all,
        "would": would
    }))
}

async fn pin(args: PinArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_pin(&args)?;
    crate::executor::require_explicit_selection("msg pin", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return pin_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            pin_core(&guard.shares(), PinParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn pin_core(
    shares: &crate::client::ServeShares,
    params: PinParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    if params.show {
        let pinned = shares
            .client
            .get_pinned_message(chat_ref)
            .await
            .map_err(tele_invocation)?;
        return Ok(serde_json::json!({
            "pinned_message": match &pinned {
                Some(m) => crate::serialize::message_to_json(m)?,
                None => serde_json::Value::Null}
        }));
    }
    if params.all {
        shares
            .client
            .unpin_all_messages(chat_ref)
            .await
            .map_err(tele_invocation)?;
        return Ok(serde_json::json!({"unpinned_all": true}));
    }
    let id = params
        .id
        .ok_or_else(|| TeleError::Usage("--id required (or use --show / --all)".to_string()))?;
    use grammers_client::tl;
    let result: std::result::Result<(), grammers_client::InvocationError> = if params.unpin {
        shares.client.unpin_message(chat_ref, id).await
    } else if params.notify {
        let input_peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
        shares
            .client
            .invoke(&tl::functions::messages::UpdatePinnedMessage {
                silent: false,
                unpin: false,
                pm_oneside: false,
                peer: input_peer,
                id,
            })
            .await
            .map(drop)
    } else {
        shares.client.pin_message(chat_ref, id).await
    };
    result.map_err(tele_invocation)?;
    Ok(serde_json::json!({"id": id, "pinned": !params.unpin}))
}

pub(crate) fn validate_pin(args: &PinArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.show || args.all {
        return Ok(());
    }
    if args.id.is_none() {
        return Err(TeleError::Usage(
            "--id required (or use --show / --all)".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_get(args: &GetArgs) -> TeleResult<()> {
    if args.chat.trim().is_empty() {
        return Err(TeleError::Usage("--chat must not be empty".into()));
    }
    if args.timeout_secs == 0 {
        return Err(TeleError::Usage("--timeout-secs must be >0".to_string()));
    }
    if args.poll_interval == 0 {
        return Err(TeleError::Usage("--poll-interval must be >=1".to_string()));
    }
    if args.last && args.offset_id.is_some() {
        return Err(TeleError::Usage(
            "--last and --offset-id are mutually exclusive".to_string(),
        ));
    }
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let target = get_target_message_id(&GetParams::from(args))?;
    if args.watch && target.is_none() {
        return Err(TeleError::Usage(
            "--watch requires --id (or a deep-link message id in --chat)".to_string(),
        ));
    }
    if target.is_some() && (args.last || args.offset_id.is_some()) {
        return Err(TeleError::Usage(
            "--last/--offset-id are mutually exclusive with a target message (--id or deep-link)"
                .to_string(),
        ));
    }
    Ok(())
}

fn get_target_message_id(params: &GetParams) -> TeleResult<Option<i32>> {
    let carried = entities::parse_target(&params.chat)?.msg_id;
    match (params.id, carried) {
        (Some(explicit), Some(link)) => Err(TeleError::Usage(format!(
            "--id {explicit} conflicts with message id {link} carried by \"{}\"; pass only one",
            params.chat
        ))),
        (explicit, link) => Ok(explicit.or(link)),
    }
}

pub(crate) fn validate_read(args: &ReadArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat").map(|_| ())
}

pub(crate) fn get_serve_dry_run(args: &GetArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "id": args.id,
        "limit": args.limit,
        "offset_id": args.offset_id,
        "last": args.last,
        "watch": args.watch,
        "timeout_secs": args.timeout_secs,
        "poll_interval": args.poll_interval,
        "would": format!("get messages from chat {}", args.chat)}))
}

fn buttons_summary(row: &serde_json::Value) -> Option<String> {
    let rows = row.get("reply_markup")?.get("rows")?.as_array()?;
    let mut parts = Vec::new();
    let mut idx = 1usize;
    for row in rows {
        if let Some(buttons) = row.as_array() {
            for btn in buttons {
                if let Some(text) = btn.get("text").and_then(|v| v.as_str()) {
                    let kind = if btn.get("url").is_some() {
                        "url"
                    } else if btn.get("callback_data").is_some() || btn.get("data").is_some() {
                        "callback"
                    } else if btn.get("switch_inline_query").is_some()
                        || btn.get("switch_inline_query_current_chat").is_some()
                    {
                        "switch_inline"
                    } else if btn.get("buy").is_some() {
                        "buy"
                    } else {
                        btn.get("raw_kind")
                            .and_then(|v| v.as_str())
                            .unwrap_or("button")
                    };
                    parts.push(format!("[{idx}] {text} ({kind})"));
                    idx += 1;
                }
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn has_buttons(rows: &[serde_json::Value]) -> bool {
    rows.iter().any(|r| buttons_summary(r).is_some())
}

fn message_table_rows(rows: &[serde_json::Value]) -> Vec<Vec<String>> {
    let include_buttons = has_buttons(rows);
    rows.iter()
        .map(|r| {
            let mut cols = vec![
                r["id"].to_string(),
                r["date"].as_str().unwrap_or_default().to_string(),
                r["sender"]["name"].as_str().unwrap_or_default().to_string(),
                truncate_text(r["text"].as_str().unwrap_or_default(), 80),
            ];
            if include_buttons {
                cols.push(buttons_summary(r).unwrap_or_default());
            }
            cols
        })
        .collect()
}

fn message_table_headers(rows: &[serde_json::Value]) -> Vec<&'static str> {
    if has_buttons(rows) {
        vec!["id", "date", "sender", "text", "buttons"]
    } else {
        vec!["id", "date", "sender", "text"]
    }
}

async fn get(args: GetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_get(&args)?;
    let config_path = flags.config_path.clone();
    let json = flags.json;
    let jsonl = flags.jsonl;
    let dry_run = flags.dry_run;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return get_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let result = if args.watch {
                get_watch_core(&guard.shares(), GetParams::from(&args)).await?
            } else {
                get_core(&guard.shares(), GetParams::from(&args)).await?
            };
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                let msgs = result["messages"].as_array().unwrap_or(&empty);
                let headers = message_table_headers(msgs);
                output::print_account_table(&name, multi, &headers, &message_table_rows(msgs))?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn get_core(
    shares: &crate::client::ServeShares,
    params: GetParams,
) -> TeleResult<serde_json::Value> {
    let target_message = get_target_message_id(&params)?;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    if let Some(target) = target_message {
        let fetched = shares
            .client
            .get_messages_by_id(chat_ref, &[target])
            .await
            .map_err(tele_invocation)?;
        let mut rows: Vec<serde_json::Value> = Vec::new();
        for msg in fetched.into_iter().flatten() {
            push_message_row(&mut rows, &msg)?;
        }
        return Ok(serde_json::json!({"messages": rows}));
    }
    let mut iter = shares.client.iter_messages(chat_ref);
    if let Some(offset) = params.offset_id {
        iter = iter.offset_id(offset);
    }
    if params.last {
        iter = iter.limit(1);
    } else {
        iter = iter.limit(params.limit as usize);
    }
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut served = 0usize;
    while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
        served += 1;
        shares.rate_limiter.acquire_for_items(served).await;
        push_message_row(&mut rows, &msg)?;
    }
    Ok(serde_json::json!({"messages": rows}))
}

fn extract_edit_date(value: &serde_json::Value) -> Option<String> {
    value
        .get("messages")?
        .as_array()?
        .first()?
        .get("edit_date")?
        .as_str()
        .map(|s| s.to_string())
}

fn extract_max_id(value: &serde_json::Value) -> Option<i32> {
    let msgs = value.get("messages")?.as_array()?;
    msgs.iter()
        .filter_map(|m| m.get("id")?.as_i64().and_then(|v| i32::try_from(v).ok()))
        .max()
}

pub(crate) async fn get_watch_core(
    shares: &crate::client::ServeShares,
    params: GetParams,
) -> TeleResult<serde_json::Value> {
    let target = get_target_message_id(&params)?.ok_or_else(|| {
        TeleError::Usage("--watch requires --id (or a deep-link message id in --chat)".to_string())
    })?;
    if params.timeout_secs == 0 {
        return Err(TeleError::Usage("--timeout-secs must be >0".to_string()));
    }
    if params.poll_interval == 0 {
        return Err(TeleError::Usage("--poll-interval must be >=1".to_string()));
    }
    let timeout = std::time::Duration::from_secs(params.timeout_secs);
    let poll = std::time::Duration::from_secs(params.poll_interval);
    let start = tokio::time::Instant::now();
    let initial = get_core(shares, params.clone()).await?;
    let initial_edit = extract_edit_date(&initial);
    if initial
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|a| a.is_empty())
        .unwrap_or(true)
    {
        return Err(TeleError::Invocation(
            format!("message {target} not found"),
            None,
        ));
    }
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return Err(TeleError::Timeout(format!(
                "watch timed out after {}s",
                params.timeout_secs
            )));
        }
        let remaining = timeout - elapsed;
        let sleep_dur = std::cmp::min(poll, remaining);
        tokio::time::sleep(sleep_dur).await;
        if start.elapsed() >= timeout {
            return Err(TeleError::Timeout(format!(
                "watch timed out after {}s",
                params.timeout_secs
            )));
        }
        let current = get_core(shares, params.clone()).await?;
        let current_edit = extract_edit_date(&current);
        if current_edit != initial_edit {
            return Ok(current);
        }
        let latest_params = GetParams {
            chat: params.chat.clone(),
            id: None,
            limit: 1,
            offset_id: None,
            last: true,
            dry_run: false,
            watch: false,
            timeout_secs: params.timeout_secs,
            poll_interval: params.poll_interval,
        };
        let latest = get_core(shares, latest_params).await?;
        if let Some(max_id) = extract_max_id(&latest) {
            if max_id > target {
                return Ok(latest);
            }
        }
    }
}

use crate::commands::helpers::truncate_text;

fn push_message_row(
    rows: &mut Vec<serde_json::Value>,
    msg: &grammers_client::message::Message,
) -> TeleResult<()> {
    let mut row = crate::serialize::message_to_json(msg)?;
    crate::serialize::enrich_message_row(&mut row, msg);
    rows.push(row);
    Ok(())
}

pub(crate) fn read_serve_dry_run(args: &ReadArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
    "dry_run": true,
    "unread": args.mark_unread,
    "mentions": args.mentions,
    "would": format!(
        "mark chat {} as {}",
        args.chat,
        if args.mentions {
            "mentions-cleared"
        } else if args.mark_unread {
            "unread"
        } else {
            "read"
        }
    )}))
}

async fn read(args: ReadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::executor::require_explicit_selection("msg read", flags)?;
    validate_read(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return read_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            read_core(&guard.shares(), ReadParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn read_core(
    shares: &crate::client::ServeShares,
    params: ReadParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    if params.mentions {
        let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
        shares
            .client
            .clear_mentions(chat_ref)
            .await
            .map_err(tele_invocation)?;
        return Ok(serde_json::json!({"mentions_cleared": true}));
    } else if params.mark_unread {
        let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
        let dialog = grammers_client::tl::enums::InputDialogPeer::Peer(
            grammers_client::tl::types::InputDialogPeer { peer },
        );
        shares
            .client
            .invoke(
                &grammers_client::tl::functions::messages::MarkDialogUnread {
                    unread: true,
                    parent_peer: None,
                    peer: dialog,
                },
            )
            .await
            .map_err(tele_invocation)?;
    } else {
        let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
        shares
            .client
            .mark_as_read(chat_ref)
            .await
            .map_err(tele_invocation)?;
    }
    Ok(serde_json::json!({"unread": params.mark_unread}))
}

pub(crate) fn validate_react(args: &ReactArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.reaction.is_some() && args.remove {
        return Err(TeleError::Usage(
            "use --reaction or --remove, not both".to_string(),
        ));
    }
    if args.reaction.is_none() && !args.remove {
        return Err(TeleError::Usage(
            "--reaction <emoji> or --remove required".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn react_serve_dry_run(args: &ReactArgs) -> TeleResult<serde_json::Value> {
    let would = if args.remove {
        format!("remove reaction from message {}", args.id)
    } else if let Some(r) = &args.reaction {
        format!("react {r} to message {}", args.id)
    } else {
        format!("react to message {}", args.id)
    };
    Ok(serde_json::json!({"dry_run": true, "id": args.id, "would": would}))
}

async fn react(args: ReactArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_react(&args)?;
    crate::executor::require_explicit_selection("msg react", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return react_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            react_core(&guard.shares(), ReactParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn react_core(
    shares: &crate::client::ServeShares,
    params: ReactParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    use grammers_client::message::InputReactions;
    let input = if params.remove {
        InputReactions::remove()
    } else if let Some(r) = &params.reaction {
        InputReactions::emoticon(r)
    } else {
        return Err(TeleError::Usage(
            "--reaction <emoji> or --remove required".to_string(),
        ));
    };
    shares
        .client
        .send_reactions(chat_ref, params.id, input)
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({"id": params.id, "reaction": params.reaction}))
}

pub(crate) fn validate_vote(args: &VoteArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    parse_vote_options(&args.option)?;
    Ok(())
}

fn parse_vote_options(spec: &str) -> TeleResult<Vec<usize>> {
    let mut seen = std::collections::BTreeSet::new();
    let mut options = Vec::new();
    for part in spec.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return Err(TeleError::Usage(format!(
                "--option must be comma-separated positive indexes like 1 or 1,2 (got {spec:?})"
            )));
        }
        let index: usize = trimmed.parse().map_err(|_| {
            TeleError::Usage(format!(
                "--option must be comma-separated positive indexes like 1 or 1,2 (got {trimmed:?})"
            ))
        })?;
        if index == 0 {
            return Err(TeleError::Usage(format!(
                "--option indexes are 1-based (got {index})"
            )));
        }
        if !seen.insert(index) {
            return Err(TeleError::Usage(format!(
                "duplicate --option index {index}"
            )));
        }
        options.push(index);
    }
    if options.is_empty() {
        return Err(TeleError::Usage(
            "--option must list at least one option index (e.g. --option 1)".to_string(),
        ));
    }
    Ok(options)
}

fn resolve_vote_options(
    answers: &[(String, Vec<u8>)],
    indexes: &[usize],
) -> TeleResult<Vec<Vec<u8>>> {
    indexes
        .iter()
        .map(|i| {
            answers
                .get(i.wrapping_sub(1))
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| {
                    TeleError::Usage(format!(
                        "--option index {i} is out of range (poll has {} option(s))",
                        answers.len()
                    ))
                })
        })
        .collect()
}

pub(crate) fn vote_serve_dry_run(args: &VoteArgs) -> TeleResult<serde_json::Value> {
    let option_indexes = parse_vote_options(&args.option)?;
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "id": args.id,
        "options": option_indexes,
        "would": format!("vote on poll {} with option(s) {option_indexes:?}", args.id)}))
}

async fn vote(args: VoteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_vote(&args)?;
    if !flags.dry_run {
        crate::executor::require_explicit_selection("msg vote", flags)?;
    }
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return vote_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            vote_core(&guard.shares(), VoteParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn vote_core(
    shares: &crate::client::ServeShares,
    params: VoteParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let id = params.id;
    let chat_target = params.chat.clone();
    let option_indexes = parse_vote_options(&params.option)?;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &chat_target).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let input_peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
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
    let poll = match msg.media() {
        Some(grammers_client::media::Media::Poll(poll)) => poll,
        _ => {
            return Err(TeleError::Invocation(
                format!("message {id} has no poll"),
                None,
            ))
        }
    };
    if poll.closed() {
        return Err(TeleError::Invocation(
            format!("poll in message {id} is closed"),
            None,
        ));
    }
    let answers = crate::serialize::poll_answers(&poll);
    let options = resolve_vote_options(&answers, &option_indexes)?;
    use grammers_client::tl;
    shares
        .client
        .invoke(&tl::functions::messages::SendVote {
            peer: input_peer,
            msg_id: id,
            options,
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({
        "id": id,
        "voted": true,
        "options": option_indexes}))
}

#[derive(Clone, Copy, Debug)]
enum TypingChoice {
    Typing,
    UploadPhoto,
    UploadFile,
    Cancel,
}

impl TypingChoice {
    fn name(self) -> &'static str {
        match self {
            TypingChoice::Typing => "typing",
            TypingChoice::UploadPhoto => "upload-photo",
            TypingChoice::UploadFile => "upload-file",
            TypingChoice::Cancel => "cancel",
        }
    }

    fn action(self) -> grammers_client::tl::enums::SendMessageAction {
        use grammers_client::tl;
        match self {
            TypingChoice::Typing => tl::enums::SendMessageAction::SendMessageTypingAction,
            TypingChoice::UploadPhoto => {
                tl::enums::SendMessageAction::SendMessageUploadPhotoAction(
                    tl::types::SendMessageUploadPhotoAction { progress: 0 },
                )
            }
            TypingChoice::UploadFile => {
                tl::enums::SendMessageAction::SendMessageUploadDocumentAction(
                    tl::types::SendMessageUploadDocumentAction { progress: 0 },
                )
            }
            TypingChoice::Cancel => tl::enums::SendMessageAction::SendMessageCancelAction,
        }
    }
}

const TYPING_ACTIONS: [&str; 4] = ["typing", "upload-photo", "upload-file", "cancel"];

fn typing_action(name: Option<&str>) -> TeleResult<TypingChoice> {
    match name
        .unwrap_or("typing")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "typing" => Ok(TypingChoice::Typing),
        "upload-photo" => Ok(TypingChoice::UploadPhoto),
        "upload-file" => Ok(TypingChoice::UploadFile),
        "cancel" => Ok(TypingChoice::Cancel),
        other => Err(TeleError::Usage(format!(
            "unknown --action {other:?} (valid actions: {TYPING_ACTIONS:?})"
        ))),
    }
}

pub(crate) fn validate_typing(args: &TypingArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    typing_action(args.action.as_deref())?;
    Ok(())
}

pub(crate) fn validate_search(args: &SearchArgs) -> TeleResult<()> {
    if args.global && !args.chat.trim().is_empty() {
        return Err(TeleError::Usage(
            "--global and --chat are mutually exclusive".to_string(),
        ));
    }
    if !args.global {
        crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    }
    if let Some(from) = args.from.as_deref() {
        crate::chat_target::ChatTarget::parse_flag(from, "from")?;
    }
    parse_search_kind(args.kind.as_deref())?;
    let since = parse_search_date("--since", args.since.as_deref())?;
    let until = parse_search_date("--until", args.until.as_deref())?;
    if let (Some(s), Some(u)) = (since, until) {
        if s > u {
            return Err(TeleError::Usage(
                "--since must not be after --until".to_string(),
            ));
        }
    }
    crate::commands::validate_limit(args.limit, 10_000, "limit").map(|_| ())
}

pub(crate) const SEARCH_KINDS: [&str; 7] =
    ["photo", "video", "gif", "document", "url", "audio", "voice"];

pub(crate) fn parse_search_kind(
    kind: Option<&str>,
) -> TeleResult<Option<grammers_client::tl::enums::MessagesFilter>> {
    use grammers_client::tl::enums::MessagesFilter;
    let Some(raw) = kind else {
        return Ok(None);
    };
    let normalized = raw.trim().to_ascii_lowercase();
    let filter = match normalized.as_str() {
        "photo" => MessagesFilter::InputMessagesFilterPhotos,
        "video" => MessagesFilter::InputMessagesFilterVideo,
        "gif" => MessagesFilter::InputMessagesFilterGif,
        "document" => MessagesFilter::InputMessagesFilterDocument,
        "url" => MessagesFilter::InputMessagesFilterUrl,
        "audio" => MessagesFilter::InputMessagesFilterMusic,
        "voice" => MessagesFilter::InputMessagesFilterVoice,
        _ => {
            return Err(TeleError::Usage(format!(
                "unknown --kind {raw:?} (valid kinds: {})",
                SEARCH_KINDS.join("|")
            )))
        }
    };
    Ok(Some(filter))
}

pub(crate) fn parse_search_date(
    flag: &str,
    value: Option<&str>,
) -> TeleResult<Option<chrono::DateTime<chrono::Utc>>> {
    value
        .map(|v| download::parse_download_date(flag, v))
        .transpose()
}

fn search_sender_matches(
    msg: &grammers_client::message::Message,
    from_id: grammers_session::types::PeerId,
) -> bool {
    match msg.sender_id() {
        Some(id) => id == from_id,
        None => msg.peer_id() == from_id,
    }
}

fn search_row_kept(
    msg: &grammers_client::message::Message,
    from_id: Option<grammers_session::types::PeerId>,
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    if let Some(want) = from_id {
        if !search_sender_matches(msg, want) {
            return false;
        }
    }
    let date = msg.date();
    if since.is_some_and(|s| date < s) {
        return false;
    }
    if until.is_some_and(|u| date > u) {
        return false;
    }
    true
}

pub(crate) fn typing_serve_dry_run(args: &TypingArgs) -> TeleResult<serde_json::Value> {
    let action_name = match typing_action(args.action.as_deref()) {
        Ok(choice) => choice.name(),
        Err(_) => "typing",
    };
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "action": action_name,
        "would": format!("send {action_name} chat action to {}", args.chat)}))
}

async fn typing(args: TypingArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_typing(&args)?;
    crate::executor::require_explicit_selection("msg typing", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return typing_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            typing_core(&guard.shares(), TypingParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn typing_core(
    shares: &crate::client::ServeShares,
    params: TypingParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let choice = typing_action(params.action.as_deref())?;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let sender = shares.client.action(chat_ref);
    match choice {
        TypingChoice::Cancel => sender.cancel().await,
        other => sender.oneshot(other.action()).await,
    }
    .map_err(tele_invocation)?;
    Ok(serde_json::json!({"chat": params.chat, "action": choice.name()}))
}

#[derive(Clone, Debug)]
enum ButtonSelector {
    Text(String),
    Index(usize),
    Contains(String),
    Data(String),
}

#[derive(Debug)]
struct LocatedButton {
    position: usize,
    text: String,
    callback_data: Option<Vec<u8>>,
    copy_text: Option<String>,
    button_type: Option<String>,
}

fn button_text(button: &serde_json::Value) -> Option<&str> {
    button.get("text").and_then(|v| v.as_str())
}

fn button_data_str(button: &serde_json::Value) -> Option<&str> {
    button
        .get("data_str")
        .and_then(|v| v.as_str())
        .or_else(|| button.get("data").and_then(|v| v.as_str()))
}

fn format_available(buttons: &[(usize, &serde_json::Value)]) -> String {
    let parts: Vec<String> = buttons
        .iter()
        .map(|(pos, b)| {
            let t = button_text(b).unwrap_or_default();
            format!("#{} {:?}", pos, t)
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

fn did_you_mean_single(buttons: &[(usize, &serde_json::Value)]) -> String {
    if let Some((pos, b)) = buttons.first() {
        let t = button_text(b).unwrap_or_default();
        format!("Did you mean #{} {:?}?", pos, t)
    } else {
        String::new()
    }
}

fn did_you_mean_pair(matches: &[(usize, &serde_json::Value)]) -> String {
    if matches.len() >= 2 {
        let (p1, b1) = matches[0];
        let (p2, b2) = matches[1];
        let t1 = button_text(b1).unwrap_or_default();
        let t2 = button_text(b2).unwrap_or_default();
        format!("Did you mean #{} {:?} or #{} {:?}?", p1, t1, p2, t2)
    } else if matches.len() == 1 {
        did_you_mean_single(matches)
    } else {
        String::new()
    }
}

fn locate_button(
    markup: &serde_json::Value,
    selector: &ButtonSelector,
) -> TeleResult<LocatedButton> {
    let kind = markup
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if kind == "reply" {
        return Err(TeleError::Usage(
            "reply-keyboard buttons are not clickable; press them by sending their text instead: tele msg send --chat <chat> --text \"<button text>\"".to_string(),
        ));
    }
    let empty = Vec::new();
    let rows = markup
        .get("rows")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let mut buttons: Vec<(usize, &serde_json::Value)> = Vec::new();
    for row in rows {
        if let Some(items) = row.as_array() {
            for button in items {
                buttons.push((buttons.len() + 1, button));
            }
        }
    }
    let total = buttons.len();
    if total == 0 {
        return Err(TeleError::Usage(
            "message has no clickable buttons".to_string(),
        ));
    }
    let all_buttons = buttons.clone();
    let matches: Vec<(usize, &serde_json::Value)> = match selector {
        ButtonSelector::Index(n) => {
            if *n == 0 {
                return Err(TeleError::Usage("--button-index is 1-based".to_string()));
            }
            buttons.into_iter().filter(|(p, _)| p == n).collect()
        }
        ButtonSelector::Text(text) => {
            let exact: Vec<_> = buttons
                .iter()
                .copied()
                .filter(|(_, b)| button_text(b) == Some(text.as_str()))
                .collect();
            if !exact.is_empty() {
                exact
            } else {
                let lowered = text.to_lowercase();
                buttons
                    .iter()
                    .copied()
                    .filter(|(_, b)| button_text(b).is_some_and(|s| s.to_lowercase() == lowered))
                    .collect()
            }
        }
        ButtonSelector::Contains(substr) => {
            let needle = substr.to_lowercase();
            buttons
                .iter()
                .copied()
                .filter(|(_, b)| button_text(b).is_some_and(|s| s.to_lowercase().contains(&needle)))
                .collect()
        }
        ButtonSelector::Data(data) => buttons
            .iter()
            .copied()
            .filter(|(_, b)| button_data_str(b) == Some(data.as_str()))
            .collect(),
    };
    match matches.len() {
        0 => Err(TeleError::Usage(match selector {
            ButtonSelector::Index(n) => {
                let available = format_available(&all_buttons);
                let suggestion = did_you_mean_single(&all_buttons);
                format!(
                    "no button at position {n} (markup has {total} button(s)). {suggestion} Available: {available}"
                )
            }
            ButtonSelector::Text(text) => {
                let available = format_available(&all_buttons);
                let suggestion = did_you_mean_single(&all_buttons);
                format!(
                    "no button named {text:?} in this message's inline keyboard. {suggestion} Available: {available}"
                )
            }
            ButtonSelector::Contains(substr) => {
                let available = format_available(&all_buttons);
                let suggestion = did_you_mean_single(&all_buttons);
                format!(
                    "no button containing {substr:?} in this message's inline keyboard. {suggestion} Available: {available}"
                )
            }
            ButtonSelector::Data(data) => {
                let available = format_available(&all_buttons);
                format!(
                    "no button with callback data {data:?} in this message's inline keyboard. Available: {available}"
                )
            }
        })),
        1 => {
            let (position, button) = matches[0];
            let text = button_text(button).unwrap_or_default().to_string();
            let button_type = button
                .get("type")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if let Some(copy) = button.get("copy_text").and_then(|v| v.as_str()) {
                return Ok(LocatedButton {
                    position,
                    text,
                    callback_data: None,
                    copy_text: Some(copy.to_string()),
                    button_type,
                });
            }
            let encoded = button
                .get("callback_data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    TeleError::Usage(format!(
                        "button {text:?} carries no callback action; only callback buttons can be clicked"
                    ))
                })?;
            use base64::engine::general_purpose::STANDARD;
            use base64::Engine;
            let callback_data = STANDARD.decode(encoded).map_err(|e| {
                TeleError::Usage(format!(
                    "button {text:?} has undecodable callback_data: {e}"
                ))
            })?;
            Ok(LocatedButton {
                position,
                text,
                callback_data: Some(callback_data),
                copy_text: None,
                button_type,
            })
        }
        _ => {
            let positions: Vec<usize> = matches.iter().map(|(p, _)| *p).collect();
            let available = format_available(&all_buttons);
            let did_mean = did_you_mean_pair(&matches);
            let label = match selector {
                ButtonSelector::Text(text) => format!("{text:?}"),
                ButtonSelector::Contains(substr) => format!("containing {substr:?}"),
                ButtonSelector::Data(data) => format!("data {data:?}"),
                ButtonSelector::Index(_) => String::new(),
            };
            Err(TeleError::Usage(format!(
                "button {label} matches multiple buttons at positions {positions:?}; use --button-index. {did_mean} Available: {available}"
            )))
        }
    }
}

fn button_requires_password(
    markup: &grammers_client::tl::enums::ReplyMarkup,
    text: &str,
    data: &[u8],
) -> bool {
    use grammers_client::tl;
    let tl::enums::ReplyMarkup::ReplyInlineMarkup(m) = markup else {
        return false;
    };
    m.rows.iter().any(|row| match row {
        tl::enums::KeyboardButtonRow::Row(r) => r.buttons.iter().any(|b| match b {
            tl::enums::KeyboardButton::Callback(cb) => {
                cb.requires_password && cb.text == text && cb.data == data
            }
            _ => false,
        }),
    })
}

pub(crate) fn validate_click(args: &ClickArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    if args.password {
        return Err(TeleError::Usage(
            "--password is not supported at this layer: grammers 0.10 exposes no public path to build the getBotCallbackAnswer SRP payload, so password-protected buttons cannot be clicked".to_string(),
        ));
    }
    let count = args.button.is_some() as u8
        + args.button_index.is_some() as u8
        + args.button_contains.is_some() as u8
        + args.button_data.is_some() as u8;
    if count == 0 {
        return Err(TeleError::Usage(
            "--button TEXT or --button-index N or --button-contains SUBSTRING or --button-data DATA required".to_string(),
        ));
    }
    if count > 1 {
        return Err(TeleError::Usage(
            "--button, --button-index, --button-contains and --button-data are mutually exclusive"
                .to_string(),
        ));
    }
    if let Some(0) = args.button_index {
        return Err(TeleError::Usage("--button-index is 1-based".to_string()));
    }
    if let Some(s) = &args.button_contains {
        if s.trim().is_empty() {
            return Err(TeleError::Usage(
                "--button-contains must not be empty".to_string(),
            ));
        }
    }
    Ok(())
}

fn click_selector(args: &ClickArgs) -> ButtonSelector {
    if let Some(idx) = args.button_index {
        ButtonSelector::Index(idx)
    } else if let Some(data) = &args.button_data {
        ButtonSelector::Data(data.clone())
    } else if let Some(substr) = &args.button_contains {
        ButtonSelector::Contains(substr.clone())
    } else if let Some(text) = &args.button {
        ButtonSelector::Text(text.clone())
    } else {
        ButtonSelector::Index(0)
    }
}

fn click_selector_label(selector: &ButtonSelector) -> String {
    match selector {
        ButtonSelector::Text(t) => format!("button {t:?}"),
        ButtonSelector::Index(n) => format!("button #{n}"),
        ButtonSelector::Contains(s) => format!("button containing {s:?}"),
        ButtonSelector::Data(d) => format!("button data {d:?}"),
    }
}

pub(crate) fn click_serve_dry_run(args: &ClickArgs) -> TeleResult<serde_json::Value> {
    let selector = click_selector(args);
    let selector_label = click_selector_label(&selector);
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "id": args.id,
        "selector": selector_label,
        "would": format!("click {selector_label} on message {}", args.id)}))
}

async fn click(args: ClickArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_click(&args)?;
    if !flags.dry_run {
        crate::executor::require_explicit_selection("msg click", flags)?;
    }
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return click_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            click_core(&guard.shares(), ClickParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn click_core(
    shares: &crate::client::ServeShares,
    params: ClickParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    if params.button.is_none()
        && params.button_index.is_none()
        && params.button_contains.is_none()
        && params.button_data.is_none()
    {
        return Err(TeleError::Usage(
            "--button/--button-index/--button-contains/--button-data required".to_string(),
        ));
    }
    let id = params.id;
    let chat_target = params.chat.clone();
    let selector = click_selector(&ClickArgs::from(&params));
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &chat_target).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let input_peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
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
    let markup = msg
        .reply_markup()
        .ok_or_else(|| TeleError::Invocation(format!("message {id} has no reply markup"), None))?;
    let markup_json = crate::serialize::reply_markup_to_json(&markup);
    let located = locate_button(&markup_json, &selector)?;
    if let Some(copy_text) = located.copy_text {
        return Ok(serde_json::json!({
            "id": id,
            "clicked": true,
            "button": located.text,
            "position": located.position,
            "type": "copy_text",
            "copy_text": copy_text,
        }));
    }
    let callback_data = located.callback_data.ok_or_else(|| {
        TeleError::Usage(format!(
            "button {:?} carries no clickable action ({} buttons are handled by their own kind, not clicks)",
            located.text, located.button_type.as_deref().unwrap_or("unknown")
        ))
    })?;
    if button_requires_password(&markup, &located.text, &callback_data) {
        return Err(TeleError::Usage(format!(
            "button {:?} requires the account's 2FA password; password-protected clicks are not supported at this layer",
            located.text
        )));
    }
    use grammers_client::tl;
    let answer: tl::enums::messages::BotCallbackAnswer = shares
        .client
        .invoke(&tl::functions::messages::GetBotCallbackAnswer {
            game: false,
            peer: input_peer,
            msg_id: id,
            data: Some(callback_data.clone()),
            password: None,
        })
        .await
        .map_err(tele_invocation)?;
    let mut row = serde_json::json!({
        "id": id,
        "clicked": true,
        "button": located.text,
        "position": located.position});
    let tl::enums::messages::BotCallbackAnswer::Answer(a) = answer;
    row["alert"] = serde_json::json!(a.alert);
    if let Some(message) = a.message {
        row["answer"] = serde_json::json!(message);
    }
    if let Some(url) = a.url {
        row["url"] = serde_json::json!(url);
    }
    row["cache_time"] = serde_json::json!(a.cache_time);
    Ok(row)
}

fn scheduled_dry_run_data(chat: &str, limit: u32) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "limit": limit,
        "would": format!("list up to {limit} scheduled messages in chat {chat}")})
}

async fn scheduled(args: ScheduledArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(scheduled_dry_run_data(args.chat.as_str(), args.limit));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            scheduled_core(&guard.shares(), ScheduledParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn scheduled_core(
    shares: &crate::client::ServeShares,
    params: ScheduledParams,
) -> TeleResult<serde_json::Value> {
    use grammers_client::tl;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let messages: tl::enums::messages::Messages = shares
        .client
        .invoke(&tl::functions::messages::GetScheduledHistory { peer, hash: 0 })
        .await
        .map_err(tele_invocation)?;
    let mut out = Vec::new();
    let (msgs, users, chats) = match messages {
        tl::enums::messages::Messages::Messages(m) => (m.messages, m.users, m.chats),
        tl::enums::messages::Messages::Slice(m) => (m.messages, m.users, m.chats),
        tl::enums::messages::Messages::ChannelMessages(m) => (m.messages, m.users, m.chats),
        tl::enums::messages::Messages::NotModified(_) => (vec![], vec![], vec![]),
    };
    for msg in msgs.iter().take(params.limit as usize) {
        if let tl::enums::Message::Message(m) = msg {
            out.push(serde_json::json!({
                "id": m.id,
                "date": m.date,
                "message": m.message,
            }));
        }
    }
    let _ = (users, chats);
    Ok(serde_json::json!({ "chat": params.chat, "scheduled": out }))
}

fn scheduled_delete_dry_run_data(chat: &str, ids: &[i32]) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "ids": ids,
        "would": format!("delete {} scheduled message(s) in chat {chat}", ids.len())})
}

async fn scheduled_delete(args: ScheduledDeleteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--ids must list at least one message id".to_string(),
        ));
    }
    crate::executor::require_explicit_selection("msg scheduled-delete", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(scheduled_delete_dry_run_data(args.chat.as_str(), &args.ids));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            scheduled_delete_core(&guard.shares(), ScheduledDeleteParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn scheduled_delete_core(
    shares: &crate::client::ServeShares,
    params: ScheduledDeleteParams,
) -> TeleResult<serde_json::Value> {
    use grammers_client::tl;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let _: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::messages::DeleteScheduledMessages {
            peer,
            id: params.ids.clone(),
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({ "chat": params.chat, "ids": params.ids, "deleted": true }))
}

fn scheduled_send_dry_run_data(chat: &str, ids: &[i32]) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "ids": ids,
        "would": format!("send {} scheduled message(s) now in chat {chat}", ids.len())})
}

async fn scheduled_send(args: ScheduledSendArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    if args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--ids must list at least one message id".to_string(),
        ));
    }
    crate::executor::require_explicit_selection("msg scheduled-send", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(scheduled_send_dry_run_data(args.chat.as_str(), &args.ids));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            scheduled_send_core(&guard.shares(), ScheduledSendParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn scheduled_send_core(
    shares: &crate::client::ServeShares,
    params: ScheduledSendParams,
) -> TeleResult<serde_json::Value> {
    use grammers_client::tl;
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    let _: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::messages::SendScheduledMessages {
            peer,
            id: params.ids.clone(),
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({ "chat": params.chat, "ids": params.ids, "sent": true }))
}

pub(crate) fn search_serve_dry_run(args: &SearchArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
    "dry_run": true,
    "chat": if args.global { serde_json::Value::Null } else { serde_json::json!(args.chat) },
    "global": args.global,
    "query": args.query,
    "limit": args.limit,
    "from": args.from,
    "kind": args.kind,
    "since": args.since,
    "until": args.until,
    "would": if args.global {
        format!("search all dialogs for \"{}\"", args.query)
    } else {
        format!("search messages in chat {}", args.chat)
    }}))
}

async fn search(args: SearchArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_search(&args)?;
    let config_path = flags.config_path.clone();
    let json = flags.json;
    let jsonl = flags.jsonl;
    let dry_run = flags.dry_run;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return search_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let result = search_core(&guard.shares(), SearchParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                let msgs = result["messages"].as_array().unwrap_or(&empty);
                let headers = message_table_headers(msgs);
                output::print_account_table(&name, multi, &headers, &message_table_rows(msgs))?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn search_core(
    shares: &crate::client::ServeShares,
    params: SearchParams,
) -> TeleResult<serde_json::Value> {
    let filter = parse_search_kind(params.kind.as_deref())?;
    let since = parse_search_date("--since", params.since.as_deref())?;
    let until = parse_search_date("--until", params.until.as_deref())?;
    if let (Some(s), Some(u)) = (since, until) {
        if s > u {
            return Err(TeleError::Usage(
                "--since must not be after --until".to_string(),
            ));
        }
    }
    let from_id = match params.from.as_deref() {
        Some(target) => Some(
            entities::resolve_peer(&shares.client, shares.session.as_ref(), target)
                .await?
                .id(),
        ),
        None => None,
    };
    shares.rate_limiter.acquire().await;
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut served = 0usize;
    let limit = params.limit as usize;
    if params.global {
        let mut iter = shares.client.search_all_messages().query(&params.query);
        if let Some(f) = filter {
            iter = iter.filter(f);
        }
        if from_id.is_none() && since.is_none() && until.is_none() {
            iter = iter.limit(limit);
            while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
                served += 1;
                shares.rate_limiter.acquire_for_items(served).await;
                push_message_row(&mut rows, &msg)?;
            }
        } else {
            while rows.len() < limit {
                match iter.next().await.map_err(tele_invocation)? {
                    Some(msg) => {
                        served += 1;
                        shares.rate_limiter.acquire_for_items(served).await;
                        if search_row_kept(&msg, from_id, since, until) {
                            push_message_row(&mut rows, &msg)?;
                        }
                    }
                    None => break,
                }
            }
        }
    } else {
        let chat =
            entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
        let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
        let mut iter = shares.client.search_messages(chat_ref).query(&params.query);
        if let Some(f) = filter {
            iter = iter.filter(f);
        }
        if let Some(s) = since {
            iter = iter.min_date(&s.into());
        }
        if let Some(u) = until {
            iter = iter.max_date(&u.into());
        }
        if let Some(want) = from_id {
            while rows.len() < limit {
                match iter.next().await.map_err(tele_invocation)? {
                    Some(msg) => {
                        served += 1;
                        shares.rate_limiter.acquire_for_items(served).await;
                        if search_sender_matches(&msg, want) {
                            push_message_row(&mut rows, &msg)?;
                        }
                    }
                    None => break,
                }
            }
        } else {
            iter = iter.limit(limit);
            while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
                served += 1;
                shares.rate_limiter.acquire_for_items(served).await;
                push_message_row(&mut rows, &msg)?;
            }
        }
    }
    Ok(serde_json::json!({"messages": rows}))
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "tests.rs"]
mod tests;

pub(crate) fn msg_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "msg click",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            false,
            "click an inline button on a bot message",
            ClickParams,
            ClickArgs,
            validate_click,
            click_serve_dry_run,
            run_click,
            crate::commands::serve::params_schema::<ClickParams>
        ),
        crate::serve_route!(
            "msg delete",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            true,
            true,
            "delete a message or all my messages in a chat",
            DeleteParams,
            DeleteArgs,
            validate_delete,
            delete_serve_dry_run,
            run_delete,
            crate::commands::serve::params_schema::<DeleteParams>
        ),
        crate::serve_route!(
            "msg download",
            Lane::Read,
            None,
            true,
            false,
            false,
            "download message media to disk",
            DownloadParams,
            DownloadArgs,
            validate_download,
            download_serve_dry_run,
            run_download,
            crate::commands::serve::params_schema::<DownloadParams>
        ),
        crate::serve_route!(
            "msg edit",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "edit the text or media of an outgoing message",
            EditParams,
            EditArgs,
            validate_edit,
            edit_serve_dry_run,
            run_edit,
            crate::commands::serve::params_schema::<EditParams>
        ),
        crate::serve_route!(
            "msg forward",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            false,
            "forward messages between chats",
            ForwardParams,
            ForwardArgs,
            validate_forward,
            forward_serve_dry_run,
            run_forward,
            crate::commands::serve::params_schema::<ForwardParams>
        ),
        crate::serve_route!(
            "msg get",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "fetch messages from a chat by recency or id",
            GetParams,
            GetArgs,
            validate_get,
            get_serve_dry_run,
            run_get,
            crate::commands::serve::params_schema::<GetParams>
        ),
        crate::serve_route!(
            "msg pin",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "pin or unpin a message in a chat",
            PinParams,
            PinArgs,
            validate_pin,
            pin_serve_dry_run,
            run_pin,
            crate::commands::serve::params_schema::<PinParams>
        ),
        crate::serve_route!(
            "msg read",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "mark a chat read up to a message",
            ReadParams,
            ReadArgs,
            validate_read,
            read_serve_dry_run,
            run_read,
            crate::commands::serve::params_schema::<ReadParams>
        ),
        crate::serve_route!(
            "msg react",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "add or remove a reaction on a message",
            ReactParams,
            ReactArgs,
            validate_react,
            react_serve_dry_run,
            run_react,
            crate::commands::serve::params_schema::<ReactParams>
        ),
        crate::serve_route!(
            "msg search",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "search messages in a chat or globally",
            SearchParams,
            SearchArgs,
            validate_search,
            search_serve_dry_run,
            run_search,
            crate::commands::serve::params_schema::<SearchParams>
        ),
        crate::serve_route!(
            "msg send",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            false,
            "send a text message to a chat",
            SendParams,
            SendArgs,
            validate_send,
            send_serve_dry_run,
            run_send,
            crate::commands::serve::params_schema::<SendParams>
        ),
        crate::serve_route!(
            "msg typing",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "send a chat action such as typing",
            TypingParams,
            TypingArgs,
            validate_typing,
            typing_serve_dry_run,
            run_typing,
            crate::commands::serve::params_schema::<TypingParams>
        ),
        crate::serve_route!(
            "msg vote",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            false,
            "vote in a poll attached to a message",
            VoteParams,
            VoteArgs,
            validate_vote,
            vote_serve_dry_run,
            run_vote,
            crate::commands::serve::params_schema::<VoteParams>
        ),
        crate::serve_route!(
            "msg scheduled",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "list scheduled messages for a chat",
            ScheduledParams,
            ScheduledArgs,
            validate_scheduled,
            scheduled_serve_dry_run,
            run_scheduled,
            crate::commands::serve::params_schema::<ScheduledParams>
        ),
        crate::serve_route!(
            "msg scheduled-delete",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "delete scheduled messages by id",
            ScheduledDeleteParams,
            ScheduledDeleteArgs,
            validate_scheduled_delete,
            scheduled_delete_serve_dry_run,
            run_scheduled_delete,
            crate::commands::serve::params_schema::<ScheduledDeleteParams>
        ),
        crate::serve_route!(
            "msg scheduled-send",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "send scheduled messages now by id",
            ScheduledSendParams,
            ScheduledSendArgs,
            validate_scheduled_send,
            scheduled_send_serve_dry_run,
            run_scheduled_send,
            crate::commands::serve::params_schema::<ScheduledSendParams>
        ),
    ]
}

crate::serve_runner!(run_send, send_core, SendParams);
crate::serve_runner!(run_edit, edit_core, EditParams);
crate::serve_runner!(run_delete, delete_core, DeleteParams);
crate::serve_runner!(run_forward, forward_core, ForwardParams);
crate::serve_runner!(run_pin, pin_core, PinParams);
crate::serve_runner!(run_get, get_core, GetParams);
crate::serve_runner!(run_read, read_core, ReadParams);
crate::serve_runner!(run_react, react_core, ReactParams);
crate::serve_runner!(run_search, search_core, SearchParams);
crate::serve_runner!(run_download, download_core, DownloadParams);
crate::serve_runner!(run_vote, vote_core, VoteParams);
crate::serve_runner!(run_typing, typing_core, TypingParams);
crate::serve_runner!(run_click, click_core, ClickParams);
crate::serve_runner!(run_scheduled, scheduled_core, ScheduledParams);
crate::serve_runner!(
    run_scheduled_delete,
    scheduled_delete_core,
    ScheduledDeleteParams
);
crate::serve_runner!(run_scheduled_send, scheduled_send_core, ScheduledSendParams);

pub(crate) fn validate_scheduled(args: &ScheduledArgs) -> TeleResult<()> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    Ok(())
}

pub(crate) fn scheduled_serve_dry_run(args: &ScheduledArgs) -> TeleResult<serde_json::Value> {
    Ok(scheduled_dry_run_data(args.chat.as_str(), args.limit))
}

pub(crate) fn validate_scheduled_delete(args: &ScheduledDeleteArgs) -> TeleResult<()> {
    if args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--ids must list at least one message id".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn scheduled_delete_serve_dry_run(
    args: &ScheduledDeleteArgs,
) -> TeleResult<serde_json::Value> {
    Ok(scheduled_delete_dry_run_data(args.chat.as_str(), &args.ids))
}

pub(crate) fn validate_scheduled_send(args: &ScheduledSendArgs) -> TeleResult<()> {
    if args.ids.is_empty() {
        return Err(TeleError::Usage(
            "--ids must list at least one message id".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn scheduled_send_serve_dry_run(
    args: &ScheduledSendArgs,
) -> TeleResult<serde_json::Value> {
    Ok(scheduled_send_dry_run_data(args.chat.as_str(), &args.ids))
}
