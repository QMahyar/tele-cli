#![allow(unused_imports)]
use grammers_client::tl;
use grammers_session::types::PeerInfo;
use grammers_session::Session;
use std::collections::HashMap;

use crate::chat_target::ChatTarget;
use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::helpers::{peer_id, stats_abs, stats_percent, stats_period};
use crate::entities;
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

use super::*;
pub(crate) fn parse_on_off(value: Option<&str>) -> TeleResult<Option<bool>> {
    match value {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(other) => Err(TeleError::Usage(format!(
            "invalid value '{other}': use on or off"
        ))),
    }
}

pub(crate) fn parse_slow_mode(value: Option<&str>) -> TeleResult<Option<i32>> {
    match value {
        None => Ok(None),
        Some("off") => Ok(Some(0)),
        Some(raw) => {
            let secs: i64 = raw.parse().map_err(|_| {
                TeleError::Usage(format!("invalid --slow-mode '{raw}': use seconds or 'off'"))
            })?;
            if !(0..=3600).contains(&secs) {
                return Err(TeleError::Usage(
                    "--slow-mode must be between 0 and 3600 seconds, or 'off'".to_string(),
                ));
            }
            Ok(Some(secs as i32))
        }
    }
}

pub(crate) fn validate_settings(args: &SettingsArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    parse_slow_mode(args.slow_mode.as_deref())?;
    parse_on_off(args.noforwards.as_deref())?;
    if let Some(value) = args.noforwards.as_deref() {
        return Err(TeleError::Usage(format!(
            "--noforwards {value} cannot be applied: the toggle method is not available in this API layer; current value is reported by read-back"
        )));
    }
    parse_on_off(args.signatures.as_deref())?;
    parse_on_off(args.pre_history.as_deref())?;
    parse_on_off(args.join_request.as_deref())?;
    Ok(())
}

pub(crate) fn channel_from_chats(
    chats: &[tl::enums::Chat],
    id: i64,
) -> Option<&tl::types::Channel> {
    for chat in chats {
        if let tl::enums::Chat::Channel(c) = chat {
            if c.id == id {
                return Some(c);
            }
        }
    }
    None
}

pub(crate) async fn settings(args: SettingsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_settings(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let slow_mode = parse_slow_mode(args.slow_mode.as_deref())?;
    let signatures = parse_on_off(args.signatures.as_deref())?;
    let pre_history = parse_on_off(args.pre_history.as_deref())?;
    let join_request = parse_on_off(args.join_request.as_deref())?;
    let has_toggles = slow_mode.is_some()
        || signatures.is_some()
        || pre_history.is_some()
        || join_request.is_some();
    if has_toggles {
        crate::executor::require_explicit_selection("chat settings", flags)?;
    }
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                let mut data = serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": if has_toggles {
                        format!("update settings of chat {target}")
                    } else {
                        format!("read settings of chat {target}")
                    }});
                if let Some(secs) = slow_mode {
                    data["slow_mode"] = serde_json::json!(secs);
                }
                if let Some(v) = signatures {
                    data["signatures"] = serde_json::json!(v);
                }
                if let Some(v) = pre_history {
                    data["pre_history"] = serde_json::json!(v);
                }
                if let Some(v) = join_request {
                    data["join_request"] = serde_json::json!(v);
                }
                return Ok(data);
            }            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "settings")?;
            let is_basic_group = matches!(&chat, grammers_client::peer::Peer::Group(_))
                && !entities::is_channel(&chat);
            if is_basic_group {
                return Err(TeleError::Usage(
                    "chat settings are not supported for basic groups; these toggles apply to channels and supergroups only".to_string(),
                ));
            }
            let input_channel = entities::input_channel(&chat).await.map_err(tele_invocation)?;
            if has_toggles {
                let mut applied = Vec::new();
                if let Some(secs) = slow_mode {
                    applied.push("slow_mode");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::ToggleSlowMode {
                            channel: input_channel.clone(),
                            seconds: secs})
                        .await
                        .map_err(tele_invocation)?;
                }
                if let Some(enabled) = signatures {
                    applied.push("signatures");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::ToggleSignatures {
                            signatures_enabled: enabled,
                            profiles_enabled: false,
                            channel: input_channel.clone()})
                        .await
                        .map_err(tele_invocation)?;
                }
                if let Some(enabled) = pre_history {
                    applied.push("pre_history");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::TogglePreHistoryHidden {
                            channel: input_channel.clone(),
                            enabled})
                        .await
                        .map_err(tele_invocation)?;
                }
                if let Some(enabled) = join_request {
                    applied.push("join_request");
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::ToggleJoinRequest {
                            apply_to_invites: enabled,
                            channel: input_channel.clone(),
                            enabled,
                            guard_bot: None})
                        .await
                        .map_err(tele_invocation)?;
                }
                return Ok(serde_json::json!({
                    "chat": target,
                    "applied": applied}));
            }
            guard.rate_limiter.acquire().await;
            let full = guard
                .client
                .invoke(&tl::functions::channels::GetFullChannel {
                    channel: input_channel})
                .await
                .map_err(tele_invocation)?;
            let tl::enums::messages::ChatFull::Full(full) = full;
            let full_chat = match full.full_chat {
                tl::enums::ChatFull::ChannelFull(f) => f,
                tl::enums::ChatFull::Full(_) => {
                    return Err(TeleError::Other(
                        "settings unavailable: server returned group info for this chat"
                            .to_string(),
                    ));
                }
            };
            let channel = channel_from_chats(&full.chats, full_chat.id);
            Ok(serde_json::json!({
                "chat": target,
                "slow_mode": full_chat.slowmode_seconds.unwrap_or(0),
                "noforwards": channel.map(|c| c.noforwards),
                "signatures": channel.map(|c| c.signatures),
                "pre_history_hidden": full_chat.hidden_prehistory,
                "join_request": channel.map(|c| c.join_request),
                "linked_chat_id": full_chat.linked_chat_id}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) const CHAT_TITLE_MAX_CHARS: usize = 128;

pub(crate) const CHAT_ABOUT_MAX_CHARS: usize = 255;

pub(crate) fn validate_edit(args: &EditArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    if args.title.is_none() && args.about.is_none() && args.photo.is_none() {
        return Err(TeleError::Usage(
            "at least one of --title, --about, --photo required".to_string(),
        ));
    }
    if let Some(title) = &args.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(TeleError::Usage("--title cannot be empty".to_string()));
        }
        if title.chars().count() > CHAT_TITLE_MAX_CHARS {
            return Err(TeleError::Usage(format!(
                "--title is too long: {} chars (max {CHAT_TITLE_MAX_CHARS})",
                title.chars().count()
            )));
        }
    }
    if let Some(about) = &args.about {
        if about.trim().chars().count() > CHAT_ABOUT_MAX_CHARS {
            return Err(TeleError::Usage(format!(
                "--about is too long: {} chars (max {CHAT_ABOUT_MAX_CHARS})",
                about.trim().chars().count()
            )));
        }
    }
    if let Some(photo) = &args.photo {
        if photo != "remove" {
            crate::commands::msg::validate_upload_path(photo)?;
        }
    }
    Ok(())
}

pub(crate) fn parse_link_target(target: Option<&str>) -> TeleResult<Option<String>> {
    match target {
        None => Ok(None),
        Some("remove") => Err(TeleError::Usage(
            "--to remove is not supported: this API layer has no unlink method (channels.setDiscussionGroup requires a group); re-point the link to another group instead".to_string(),
        )),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(TeleError::Usage("--to cannot be empty".to_string()));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

pub(crate) fn validate_link(args: &LinkArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    parse_link_target(args.to.as_deref())?;
    Ok(())
}

pub(crate) fn chat_photo_input_photo(photo: &tl::enums::Photo) -> Option<tl::enums::InputPhoto> {
    if let tl::enums::Photo::Photo(p) = photo {
        return Some(tl::enums::InputPhoto::Photo(tl::types::InputPhoto {
            id: p.id,
            access_hash: p.access_hash,
            file_reference: p.file_reference.clone(),
        }));
    }
    None
}

pub(crate) async fn fetch_full_chat_info(
    client: &grammers_client::Client,
    chat: &grammers_client::peer::Peer,
) -> TeleResult<tl::enums::messages::ChatFull> {
    if matches!(chat, grammers_client::peer::Peer::Group(_)) && !entities::is_channel(chat) {
        client
            .invoke(&tl::functions::messages::GetFullChat {
                chat_id: chat.id().bare_id().unwrap_or_default(),
            })
            .await
            .map_err(tele_invocation)
    } else {
        let input_channel = entities::input_channel(chat)
            .await
            .map_err(tele_invocation)?;
        client
            .invoke(&tl::functions::channels::GetFullChannel {
                channel: input_channel,
            })
            .await
            .map_err(tele_invocation)
    }
}

pub(crate) async fn edit_chat(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_edit(&args)?;
    crate::executor::require_explicit_selection("chat edit", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let title = args.title.clone();
    let about = args.about.clone();
    let photo = args.photo.clone();
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let title = title.clone();
        let about = about.clone();
        let photo = photo.clone();
        Box::pin(async move {
            if dry_run {
                let mut data = serde_json::json!({
                    "dry_run": true,
                    "chat": target,
                    "would": format!("edit metadata of chat {target}")});
                if let Some(t) = &title {
                    data["title"] = serde_json::json!(t.trim());
                }
                if let Some(a) = &about {
                    data["about"] = serde_json::json!(a.trim());
                }
                if let Some(p) = &photo {
                    data["photo"] = serde_json::json!(p);
                }
                return Ok(data);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "chat edit")?;
            let is_basic_group = matches!(&chat, grammers_client::peer::Peer::Group(_))
                && !entities::is_channel(&chat);
            let mut applied = Vec::new();
            if let Some(new_title) = &title {
                applied.push("title");
                let new_title = new_title.trim().to_string();
                if is_basic_group {
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::messages::EditChatTitle {
                            chat_id: chat.id().bare_id().unwrap_or_default(),
                            title: new_title,
                        })
                        .await
                        .map_err(tele_invocation)?;
                } else {
                    guard.rate_limiter.acquire().await;
                    guard
                        .client
                        .invoke(&tl::functions::channels::EditTitle {
                            channel: entities::input_channel(&chat)
                                .await
                                .map_err(tele_invocation)?,
                            title: new_title,
                        })
                        .await
                        .map_err(tele_invocation)?;
                }
            }
            if let Some(new_about) = &about {
                applied.push("about");
                let new_about = new_about.trim().to_string();
                let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
                guard.rate_limiter.acquire().await;
                guard
                    .client
                    .invoke(&tl::functions::messages::EditChatAbout {
                        peer,
                        about: new_about,
                    })
                    .await
                    .map_err(tele_invocation)?;
            }
            if let Some(photo) = &photo {
                applied.push("photo");
                if photo == "remove" {
                    let full = fetch_full_chat_info(&guard.client, &chat).await?;
                    let tl::enums::messages::ChatFull::Full(full) = full;
                    let current: Option<tl::enums::Photo> = match &full.full_chat {
                        tl::enums::ChatFull::ChannelFull(f) => Some(f.chat_photo.clone()),
                        tl::enums::ChatFull::Full(f) => f.chat_photo.clone(),
                    };
                    let input_photo = current
                        .as_ref()
                        .and_then(chat_photo_input_photo)
                        .ok_or_else(|| {
                            TeleError::Other("chat has no photo to remove".to_string())
                        })?;
                    guard.rate_limiter.acquire().await;
                    let _: Vec<i64> = guard
                        .client
                        .invoke(&tl::functions::photos::DeletePhotos {
                            id: vec![input_photo],
                        })
                        .await
                        .map_err(tele_invocation)?;
                } else {
                    let uploaded = guard
                        .client
                        .upload_file(photo)
                        .await
                        .map_err(|e| TeleError::TaskPanic(e.to_string()))?;
                    let chat_photo = tl::enums::InputChatPhoto::InputChatUploadedPhoto(
                        tl::types::InputChatUploadedPhoto {
                            file: Some(uploaded.raw),
                            video: None,
                            video_start_ts: None,
                            video_emoji_markup: None,
                        },
                    );
                    guard.rate_limiter.acquire().await;
                    if is_basic_group {
                        guard
                            .client
                            .invoke(&tl::functions::messages::EditChatPhoto {
                                chat_id: chat.id().bare_id().unwrap_or_default(),
                                photo: chat_photo,
                            })
                            .await
                            .map_err(tele_invocation)?;
                    } else {
                        guard
                            .client
                            .invoke(&tl::functions::channels::EditPhoto {
                                channel: entities::input_channel(&chat)
                                    .await
                                    .map_err(tele_invocation)?,
                                photo: chat_photo,
                            })
                            .await
                            .map_err(tele_invocation)?;
                    }
                }
            }
            Ok(serde_json::json!({
                "chat": target,
                "applied": applied}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) fn discussion_pair(
    x: grammers_client::peer::Peer,
    y: grammers_client::peer::Peer,
) -> TeleResult<(grammers_client::peer::Peer, grammers_client::peer::Peer)> {
    pub(crate) const NOT_A_DISCUSSION_PEER: &str =
        "discussion links need one broadcast channel and one supergroup";
    let x_broadcast = match &x {
        grammers_client::peer::Peer::Channel(c) => Ok(c.raw.broadcast),
        grammers_client::peer::Peer::Group(g) => match &g.raw {
            tl::enums::Chat::Channel(c) => Ok(c.broadcast),
            tl::enums::Chat::ChannelForbidden(c) => Ok(c.broadcast),
            _ => Err(TeleError::Usage(NOT_A_DISCUSSION_PEER.to_string())),
        },
        _ => Err(TeleError::Usage(NOT_A_DISCUSSION_PEER.to_string())),
    }?;
    let y_broadcast = match &y {
        grammers_client::peer::Peer::Channel(c) => Ok(c.raw.broadcast),
        grammers_client::peer::Peer::Group(g) => match &g.raw {
            tl::enums::Chat::Channel(c) => Ok(c.broadcast),
            tl::enums::Chat::ChannelForbidden(c) => Ok(c.broadcast),
            _ => Err(TeleError::Usage(NOT_A_DISCUSSION_PEER.to_string())),
        },
        _ => Err(TeleError::Usage(NOT_A_DISCUSSION_PEER.to_string())),
    }?;
    match (x_broadcast, y_broadcast) {
        (true, false) => Ok((x, y)),
        (false, true) => Ok((y, x)),
        _ => Err(TeleError::Usage(format!(
            "--chat and --to must be one broadcast channel and one supergroup (got {} + {})",
            if x_broadcast {
                "broadcast"
            } else {
                "supergroup"
            },
            if y_broadcast {
                "broadcast"
            } else {
                "supergroup"
            }
        ))),
    }
}

pub(crate) async fn link_chat(args: LinkArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_link(&args)?;
    let to_target = parse_link_target(args.to.as_deref())?;
    if to_target.is_some() {
        crate::executor::require_explicit_selection("chat link", flags)?;
    }
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let to_target = parse_link_target(args.to.as_deref())?;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        let to_target = to_target.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(match &to_target {
                    None => serde_json::json!({
                        "dry_run": true,
                        "chat": target,
                        "would": format!("read discussion link of chat {target}")}),
                    Some(to) => serde_json::json!({
                        "dry_run": true,
                        "chat": target,
                        "to": to,
                        "would": format!("link chat {target} with discussion group {to}")}),
                });
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &target).await?;
            ensure_chat_peer(&chat, "link")?;
            let Some(to_target) = to_target else {
                let full = fetch_full_chat_info(&guard.client, &chat).await?;
                let tl::enums::messages::ChatFull::Full(full) = full;
                let linked = match full.full_chat {
                    tl::enums::ChatFull::ChannelFull(f) => f.linked_chat_id,
                    tl::enums::ChatFull::Full(_) => None,
                };
                return Ok(serde_json::json!({
                    "chat": target,
                    "linked_chat_id": linked}));
            };
            let to_peer =
                entities::resolve_peer(&guard.client, guard.session.as_ref(), &to_target).await?;
            ensure_chat_peer(&to_peer, "--to")?;
            let (broadcast, group) = discussion_pair(chat.clone(), to_peer)?;
            guard.rate_limiter.acquire().await;
            guard
                .client
                .invoke(&tl::functions::channels::SetDiscussionGroup {
                    broadcast: entities::input_channel(&broadcast)
                        .await
                        .map_err(tele_invocation)?,
                    group: entities::input_channel(&group)
                        .await
                        .map_err(tele_invocation)?,
                })
                .await
                .map_err(tele_invocation)?;
            Ok(serde_json::json!({
                "chat": target,
                "to": to_target,
                "linked": true}))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}
