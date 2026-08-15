use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::account::tele_invocation;
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum ProfileCmd {
    Get(GetArgs),
    Set(SetArgs),
}

#[derive(Args)]
pub struct GetArgs {
    #[arg(long, help = "target user: @username, numeric ID, or me (default)")]
    chat: Option<String>,
}

#[derive(Args)]
pub struct SetArgs {
    #[arg(long, help = "new display name (first and last)")]
    name: Option<String>,
    #[arg(long, help = "new bio/about text")]
    bio: Option<String>,
    #[arg(long, help = "path to new profile photo")]
    photo: Option<String>,
}

pub async fn run(cmd: ProfileCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        ProfileCmd::Get(a) => get(a, flags).await,
        ProfileCmd::Set(a) => set(a, flags).await,
    }
}

async fn get(args: GetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let target = args.chat.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(get_dry_run_payload(target.as_deref()));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let row = match &target {
                Some(t) => {
                    let peer = entities::resolve_peer(&guard.client, guard.session.as_ref(), t)
                        .await
                        .map_err(tele_invocation)?;
                    match &peer {
                        grammers_client::peer::Peer::User(user) => {
                            let input =
                                entities::input_user(&peer).await.map_err(tele_invocation)?;
                            let full: tl::enums::users::UserFull = guard
                                .client
                                .invoke(&tl::functions::users::GetFullUser { id: input })
                                .await
                                .map_err(tele_invocation)?;
                            let tl::enums::users::UserFull::Full(full) = full;
                            let tl::enums::UserFull::Full(full_user) = full.full_user;
                            serde_json::json!({
                                "id": user.id().bare_id().unwrap_or_default(),
                                "name": user.full_name(),
                                "username": user.username().unwrap_or_default(),
                                "phone": user.phone().unwrap_or_default(),
                                "bio": full_user.about,
                                "bot": user.is_bot(),
                            })
                        }
                        other => serde_json::json!({
                            "id": other.id().bare_id().unwrap_or_default(),
                            "name": crate::serialize::peer_name(other),
                            "kind": crate::serialize::peer_kind(other),
                        }),
                    }
                }
                None => {
                    let me = guard.client.get_me().await.map_err(tele_invocation)?;
                    let input =
                        entities::input_user(&grammers_client::peer::Peer::User(me.clone()))
                            .await
                            .map_err(tele_invocation)?;
                    let full: tl::enums::users::UserFull = guard
                        .client
                        .invoke(&tl::functions::users::GetFullUser { id: input })
                        .await
                        .map_err(tele_invocation)?;
                    let tl::enums::users::UserFull::Full(full) = full;
                    let tl::enums::UserFull::Full(full_user) = full.full_user;
                    serde_json::json!({
                        "id": me.id().bare_id().unwrap_or_default(),
                        "name": me.full_name(),
                        "username": me.username().unwrap_or_default(),
                        "phone": me.phone().unwrap_or_default(),
                        "bio": full_user.about,
                        "bot": me.is_bot(),
                    })
                }
            };
            if !output::machine_mode(json, jsonl) {
                for (k, v) in row.as_object().unwrap() {
                    println!("{k}: {v}");
                }
            }
            Ok(row)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_set(args: &SetArgs) -> TeleResult<()> {
    if args.name.is_none() && args.bio.is_none() && args.photo.is_none() {
        return Err(TeleError::Usage(
            "at least one of --name, --bio, --photo required".to_string(),
        ));
    }
    Ok(())
}

fn get_dry_run_payload(target: Option<&str>) -> serde_json::Value {
    match target {
        Some(t) => serde_json::json!({"dry_run": true, "chat": t}),
        None => serde_json::json!({"dry_run": true}),
    }
}

async fn set(args: SetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_set(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let new_name = args.name.clone();
        let new_bio = args.bio.clone();
        let photo = args.photo.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({"dry_run": true}));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            if new_name.is_some() || new_bio.is_some() {
                let (first, last) = match &new_name {
                    Some(n) => {
                        let mut parts = n.splitn(2, ' ');
                        (
                            Some(parts.next().unwrap_or(n).to_string()),
                            Some(parts.next().unwrap_or("").to_string()),
                        )
                    }
                    None => (None, None),
                };
                let _: tl::enums::User = guard
                    .client
                    .invoke(&tl::functions::account::UpdateProfile {
                        first_name: first,
                        last_name: last,
                        about: new_bio.clone(),
                    })
                    .await
                    .map_err(tele_invocation)?;
            }
            if let Some(p) = &photo {
                let uploaded = guard
                    .client
                    .upload_file(p)
                    .await
                    .map_err(|e| TeleError::Other(e.to_string()))?;
                let uploaded_photo: tl::enums::photos::Photo = guard
                    .client
                    .invoke(&tl::functions::photos::UploadProfilePhoto {
                        fallback: false,
                        bot: None,
                        file: Some(uploaded.raw),
                        video: None,
                        video_start_ts: None,
                        video_emoji_markup: None,
                    })
                    .await
                    .map_err(tele_invocation)?;
                let tl::enums::photos::Photo::Photo(uploaded_photo) = uploaded_photo;
                let tl::enums::Photo::Photo(photo) = uploaded_photo.photo else {
                    return Err(TeleError::Other(
                        "profile photo upload returned an empty photo".to_string(),
                    ));
                };
                let _: tl::enums::photos::Photo = guard
                    .client
                    .invoke(&tl::functions::photos::UpdateProfilePhoto {
                        fallback: false,
                        bot: None,
                        id: tl::enums::InputPhoto::Photo(tl::types::InputPhoto {
                            id: photo.id,
                            access_hash: photo.access_hash,
                            file_reference: photo.file_reference,
                        }),
                    })
                    .await
                    .map_err(tele_invocation)?;
            }
            Ok(serde_json::json!({
                "name": new_name,
                "bio": new_bio,
                "photo": photo,
            }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_requires_at_least_one_flag() {
        let none = SetArgs {
            name: None,
            bio: None,
            photo: None,
        };
        assert!(matches!(validate_set(&none), Err(TeleError::Usage(_))));
        let with_bio = SetArgs {
            name: None,
            bio: Some("b".to_string()),
            photo: None,
        };
        assert!(validate_set(&with_bio).is_ok());
        let with_name = SetArgs {
            name: Some("n".to_string()),
            bio: None,
            photo: None,
        };
        assert!(validate_set(&with_name).is_ok());
    }

    #[test]
    fn get_dry_run_payload_marks_dry_run_only() {
        let v = get_dry_run_payload(None);
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert!(v.get("chat").is_none());
    }

    #[test]
    fn get_dry_run_payload_carries_chat_target() {
        let v = get_dry_run_payload(Some("me"));
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["chat"], serde_json::json!("me"));
    }
}
