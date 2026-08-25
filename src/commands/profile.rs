use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::msg::validate_upload_path;
use crate::entities;
use crate::error::tele_invocation;
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum ProfileCmd {
    Get(GetArgs),
    Set(SetArgs),
    Photo(PhotoArgs),
    EmojiStatus(EmojiStatusArgs),
}

#[derive(Args, Clone)]
pub struct GetArgs {
    #[arg(long, help = "target user: @username, numeric ID, or me (default)")]
    chat: Option<String>,
    #[arg(long, help = "include the account phone number (redacted by default)")]
    show_phone: bool,
}

#[derive(Args, Clone)]
pub struct SetArgs {
    #[arg(long, help = "new display name (first and last)")]
    name: Option<String>,
    #[arg(long, help = "new bio/about text")]
    bio: Option<String>,
    #[arg(long, help = "path to new profile photo")]
    photo: Option<String>,
    #[arg(long, help = "new username (5-32 chars) or 'remove' to clear it")]
    username: Option<String>,
}

#[derive(Args, Clone)]
pub struct PhotoArgs {
    #[arg(long, help = "remove the current profile photo")]
    remove: bool,
}

#[derive(Args, Clone)]
pub struct EmojiStatusArgs {
    #[arg(long, help = "custom emoji document ID to set as emoji status")]
    emoji: Option<i64>,
    #[arg(long, help = "clear the emoji status")]
    remove: bool,
}

pub async fn run(cmd: ProfileCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        ProfileCmd::Get(a) => get(a, flags).await,
        ProfileCmd::Set(a) => set(a, flags).await,
        ProfileCmd::Photo(a) => photo(a, flags).await,
        ProfileCmd::EmojiStatus(a) => emoji_status(a, flags).await,
    }
}

async fn get(args: GetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
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
            let row = get_core(&guard.shares(), GetParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                for (k, v) in row.as_object().unwrap() {
                    output::print_line(&format!("{k}: {v}"))?;
                }
            }
            Ok(row)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn validate_set(args: &SetArgs) -> TeleResult<()> {
    if args.name.is_none() && args.bio.is_none() && args.photo.is_none() && args.username.is_none()
    {
        return Err(TeleError::Usage(
            "at least one of --name, --bio, --photo, --username required".to_string(),
        ));
    }
    if let Some(name) = &args.name {
        if name.trim().is_empty() {
            return Err(TeleError::Usage("name must not be empty".to_string()));
        }
        let (first, last) = split_full_name(name);
        if let Some(first) = &first {
            if first.chars().count() > MAX_NAME_CHARS {
                return Err(TeleError::Usage(
                    "first name exceeds 64 characters".to_string(),
                ));
            }
        }
        if let Some(last) = &last {
            if last.chars().count() > MAX_NAME_CHARS {
                return Err(TeleError::Usage(
                    "last name exceeds 64 characters".to_string(),
                ));
            }
        }
    }
    if let Some(bio) = &args.bio {
        if bio.trim().chars().count() > MAX_BIO_CHARS {
            return Err(TeleError::Usage("bio exceeds 140 characters".to_string()));
        }
    }
    if let Some(path) = &args.photo {
        validate_upload_path(path)?;
    }
    validate_username_arg(args.username.as_deref())?;
    Ok(())
}

fn validate_username_arg(raw: Option<&str>) -> TeleResult<()> {
    let Some(raw) = raw else {
        return Ok(());
    };
    if raw.trim().eq_ignore_ascii_case("remove") {
        return Ok(());
    }
    validate_username(strip_username_prefixes(raw.trim()))
}

fn strip_username_prefixes(raw: &str) -> &str {
    let mut s = raw;
    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
        }
    }
    for prefix in ["t.me/", "telegram.me/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
        }
    }
    s.strip_prefix('@').unwrap_or(s)
}

fn validate_username(username: &str) -> TeleResult<()> {
    let err = |why: String| TeleError::Usage(format!("invalid --username {username}: {why}"));
    if username.is_empty() {
        return Err(err("must not be empty".to_string()));
    }
    if !(MIN_USERNAME_CHARS..=MAX_USERNAME_CHARS).contains(&username.chars().count()) {
        return Err(err(format!(
            "length must be {MIN_USERNAME_CHARS}-{MAX_USERNAME_CHARS} characters"
        )));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(err(
            "only letters, digits and underscores are allowed".to_string()
        ));
    }
    if username.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(err("must not start with a digit".to_string()));
    }
    if username.ends_with('_') {
        return Err(err("must not end with an underscore".to_string()));
    }
    if !username.chars().any(char::is_alphabetic) {
        return Err(err("must contain at least one letter".to_string()));
    }
    Ok(())
}

const MAX_NAME_CHARS: usize = 64;
const MAX_BIO_CHARS: usize = 140;
const MIN_USERNAME_CHARS: usize = 5;
const MAX_USERNAME_CHARS: usize = 32;

fn split_full_name(raw: &str) -> (Option<String>, Option<String>) {
    let trimmed = raw.trim();
    match trimmed.split_once(' ') {
        Some((first, last)) => {
            let last = last.trim();
            (
                Some(first.trim().to_string()),
                if last.is_empty() {
                    None
                } else {
                    Some(last.to_string())
                },
            )
        }
        None => (Some(trimmed.to_string()), None),
    }
}

fn redact_phone(phone: Option<&str>, show_phone: bool) -> Option<&str> {
    if show_phone {
        phone
    } else {
        None
    }
}

fn get_dry_run_payload(target: Option<&str>) -> serde_json::Value {
    match target {
        Some(t) => serde_json::json!({
            "dry_run": true,
            "chat": t,
            "would": format!("get profile of user {t}")
        }),
        None => serde_json::json!({
            "dry_run": true,
            "would": "get own profile"
        }),
    }
}

fn username_rpc_error(e: grammers_client::InvocationError) -> TeleError {
    if let grammers_client::InvocationError::Rpc(rpc) = &e {
        match rpc.name.as_str() {
            "USERNAME_NOT_ALLOWED" => {
                return TeleError::Usage(
                    "username rejected: USERNAME_NOT_ALLOWED (this account cannot claim usernames; \
                     Telegram may require an active account or premium)"
                        .to_string(),
                );
            }
            "USERNAME_INVALID" | "USERNAME_BAD_SYNTAX" => {
                return TeleError::Usage(
                    "username rejected: USERNAME_INVALID (bad characters or shape)".to_string(),
                );
            }
            "USERNAME_OCCUPIED" => {
                return TeleError::Usage(
                    "username rejected: USERNAME_OCCUPIED (already taken)".to_string(),
                );
            }
            _ => {}
        }
    }
    tele_invocation(e)
}

async fn set(args: SetArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_set(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return set_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            set_core(&guard.shares(), SetParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn apply_username(shares: &crate::client::ServeShares, raw: &str) -> TeleResult<String> {
    let trimmed = raw.trim();
    let removing = trimmed.eq_ignore_ascii_case("remove");
    let value = if removing {
        String::new()
    } else {
        strip_username_prefixes(trimmed).to_string()
    };
    shares.rate_limiter.acquire().await;
    let _: tl::enums::User = shares
        .client
        .invoke(&tl::functions::account::UpdateUsername {
            username: value.clone(),
        })
        .await
        .map_err(username_rpc_error)?;
    if removing {
        Ok("removed".to_string())
    } else {
        Ok(value)
    }
}

async fn photo(args: PhotoArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_photo(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return photo_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            photo_core(&guard.shares(), PhotoParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn current_photo_input_photo(
    shares: &crate::client::ServeShares,
) -> TeleResult<tl::enums::InputPhoto> {
    let me = shares.client.get_me().await.map_err(tele_invocation)?;
    let input = entities::input_user(&grammers_client::peer::Peer::User(me.clone()))
        .await
        .map_err(tele_invocation)?;
    let full: tl::enums::users::UserFull = shares
        .client
        .invoke(&tl::functions::users::GetFullUser { id: input })
        .await
        .map_err(tele_invocation)?;
    let tl::enums::users::UserFull::Full(full) = full;
    let tl::enums::UserFull::Full(full_user) = full.full_user;
    let Some(photo) = &full_user.profile_photo else {
        return Err(TeleError::Other(
            "profile has no photo to remove".to_string(),
        ));
    };
    crate::commands::chat::chat_photo_input_photo(photo)
        .ok_or_else(|| TeleError::Other("profile has no removable photo to remove".to_string()))
}

async fn remove_profile_photo(shares: &crate::client::ServeShares) -> TeleResult<()> {
    let input = current_photo_input_photo(shares).await?;
    shares.rate_limiter.acquire().await;
    let _: Vec<i64> = shares
        .client
        .invoke(&tl::functions::photos::DeletePhotos { id: vec![input] })
        .await
        .map_err(tele_invocation)?;
    Ok(())
}

fn validate_emoji_status(args: &EmojiStatusArgs) -> TeleResult<Option<i64>> {
    match (args.emoji, args.remove) {
        (Some(id), false) => {
            if id <= 0 {
                return Err(TeleError::Usage(format!(
                    "--emoji must be a positive document ID; got {id}"
                )));
            }
            Ok(Some(id))
        }
        (None, true) => Ok(None),
        (Some(_), true) => Err(TeleError::Usage(
            "--emoji and --remove are mutually exclusive".to_string(),
        )),
        (None, false) => Err(TeleError::Usage(
            "emoji-status requires --emoji <document-id> or --remove".to_string(),
        )),
    }
}

async fn emoji_status(args: EmojiStatusArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_emoji_status(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return emoji_status_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            emoji_status_core(&guard.shares(), EmojiStatusParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetParams {
    #[serde(default)]
    pub(crate) chat: Option<String>,
    #[serde(default)]
    pub(crate) show_phone: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&GetArgs> for GetParams {
    fn from(a: &GetArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            show_phone: a.show_phone,
            dry_run: false,
        }
    }
}

impl From<&GetParams> for GetArgs {
    fn from(p: &GetParams) -> Self {
        Self {
            chat: p.chat.clone(),
            show_phone: p.show_phone,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetParams {
    pub(crate) name: Option<String>,
    pub(crate) bio: Option<String>,
    pub(crate) photo: Option<String>,
    pub(crate) username: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&SetArgs> for SetParams {
    fn from(a: &SetArgs) -> Self {
        Self {
            name: a.name.clone(),
            bio: a.bio.clone(),
            photo: a.photo.clone(),
            username: a.username.clone(),
            dry_run: false,
        }
    }
}

impl From<&SetParams> for SetArgs {
    fn from(p: &SetParams) -> Self {
        Self {
            name: p.name.clone(),
            bio: p.bio.clone(),
            photo: p.photo.clone(),
            username: p.username.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoParams {
    #[serde(default)]
    pub(crate) remove: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&PhotoArgs> for PhotoParams {
    fn from(a: &PhotoArgs) -> Self {
        Self {
            remove: a.remove,
            dry_run: false,
        }
    }
}

impl From<&PhotoParams> for PhotoArgs {
    fn from(p: &PhotoParams) -> Self {
        Self { remove: p.remove }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmojiStatusParams {
    pub(crate) emoji: Option<i64>,
    #[serde(default)]
    pub(crate) remove: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&EmojiStatusArgs> for EmojiStatusParams {
    fn from(a: &EmojiStatusArgs) -> Self {
        Self {
            emoji: a.emoji,
            remove: a.remove,
            dry_run: false,
        }
    }
}

impl From<&EmojiStatusParams> for EmojiStatusArgs {
    fn from(p: &EmojiStatusParams) -> Self {
        Self {
            emoji: p.emoji,
            remove: p.remove,
        }
    }
}

fn validate_get(_args: &GetArgs) -> TeleResult<()> {
    Ok(())
}

fn validate_photo(args: &PhotoArgs) -> TeleResult<()> {
    if !args.remove {
        return Err(TeleError::Usage(
            "profile photo requires --remove (setting a photo is profile set --photo <path>)"
                .to_string(),
        ));
    }
    Ok(())
}

fn get_serve_dry_run(args: &GetArgs) -> TeleResult<serde_json::Value> {
    Ok(get_dry_run_payload(args.chat.as_deref()))
}

fn set_serve_dry_run(args: &SetArgs) -> TeleResult<serde_json::Value> {
    let mut fields = Vec::new();
    if args.name.is_some() {
        fields.push("name");
    }
    if args.bio.is_some() {
        fields.push("bio");
    }
    if args.photo.is_some() {
        fields.push("photo");
    }
    if args.username.is_some() {
        fields.push("username");
    }
    Ok(serde_json::json!({
        "dry_run": true,
        "would": format!("set profile {}", fields.join(", "))
    }))
}

fn photo_serve_dry_run(_args: &PhotoArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "would": "remove current profile photo"
    }))
}

fn emoji_status_serve_dry_run(args: &EmojiStatusArgs) -> TeleResult<serde_json::Value> {
    Ok(match validate_emoji_status(args)? {
        Some(id) => serde_json::json!({
            "dry_run": true,
            "would": format!("set emoji status to emoji document {id}")
        }),
        None => serde_json::json!({
            "dry_run": true,
            "would": "clear emoji status"
        }),
    })
}

pub(crate) async fn get_core(
    shares: &crate::client::ServeShares,
    params: GetParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    match &params.chat {
        Some(t) => {
            let peer = entities::resolve_peer(&shares.client, shares.session.as_ref(), t).await?;
            match &peer {
                grammers_client::peer::Peer::User(user) => {
                    let input = entities::input_user(&peer).await.map_err(tele_invocation)?;
                    let full: tl::enums::users::UserFull = shares
                        .client
                        .invoke(&tl::functions::users::GetFullUser { id: input })
                        .await
                        .map_err(tele_invocation)?;
                    let tl::enums::users::UserFull::Full(full) = full;
                    let tl::enums::UserFull::Full(full_user) = full.full_user;
                    Ok(serde_json::json!({
                        "id": user.id().bare_id().unwrap_or_default(),
                        "name": user.full_name(),
                        "username": user.username().unwrap_or_default(),
                        "phone": redact_phone(user.phone(), params.show_phone),
                        "bio": full_user.about,
                        "bot": user.is_bot(),
                    }))
                }
                other => Ok(serde_json::json!({
                    "id": other.id().bare_id().unwrap_or_default(),
                    "name": crate::serialize::peer_name(other),
                    "kind": crate::serialize::peer_kind(other),
                })),
            }
        }
        None => {
            let me = shares.client.get_me().await.map_err(tele_invocation)?;
            let input = entities::input_user(&grammers_client::peer::Peer::User(me.clone()))
                .await
                .map_err(tele_invocation)?;
            let full: tl::enums::users::UserFull = shares
                .client
                .invoke(&tl::functions::users::GetFullUser { id: input })
                .await
                .map_err(tele_invocation)?;
            let tl::enums::users::UserFull::Full(full) = full;
            let tl::enums::UserFull::Full(full_user) = full.full_user;
            Ok(serde_json::json!({
                "id": me.id().bare_id().unwrap_or_default(),
                "name": me.full_name(),
                "username": me.username().unwrap_or_default(),
                "phone": redact_phone(me.phone(), params.show_phone),
                "bio": full_user.about,
                "bot": me.is_bot(),
            }))
        }
    }
}

pub(crate) async fn set_core(
    shares: &crate::client::ServeShares,
    params: SetParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let new_name = params.name.clone();
    let new_bio = params.bio.clone();
    let photo_path = params.photo.clone();
    let username_raw = params.username.clone();
    if new_name.is_some() || new_bio.is_some() {
        let (first, last) = match &new_name {
            Some(n) => split_full_name(n),
            None => (None, None),
        };
        let _: tl::enums::User = shares
            .client
            .invoke(&tl::functions::account::UpdateProfile {
                first_name: first,
                last_name: last,
                about: new_bio.clone(),
            })
            .await
            .map_err(tele_invocation)?;
    }
    if new_name.is_some() || new_bio.is_some() || photo_path.is_some() {
        shares.rate_limiter.acquire().await;
    }
    let mut applied_username: Option<String> = None;
    if let Some(raw) = &username_raw {
        applied_username = Some(apply_username(shares, raw).await?);
    }
    if let Some(p) = &photo_path {
        let uploaded = shares
            .client
            .upload_file(p)
            .await
            .map_err(|e| TeleError::TaskPanic(e.to_string()))?;
        let uploaded_photo: tl::enums::photos::Photo = shares
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
        let _: tl::enums::photos::Photo = shares
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
        "photo": photo_path,
        "username": applied_username,
    }))
}

pub(crate) async fn photo_core(
    shares: &crate::client::ServeShares,
    _params: PhotoParams,
) -> TeleResult<serde_json::Value> {
    remove_profile_photo(shares).await?;
    Ok(serde_json::json!({"removed": true}))
}

pub(crate) async fn emoji_status_core(
    shares: &crate::client::ServeShares,
    params: EmojiStatusParams,
) -> TeleResult<serde_json::Value> {
    let document_id = validate_emoji_status(&EmojiStatusArgs::from(&params))?;
    shares.rate_limiter.acquire().await;
    let status = match document_id {
        Some(id) => tl::enums::EmojiStatus::Status(tl::types::EmojiStatus {
            document_id: id,
            until: None,
        }),
        None => tl::enums::EmojiStatus::Empty,
    };
    let _: bool = shares
        .client
        .invoke(&tl::functions::account::UpdateEmojiStatus {
            emoji_status: status,
        })
        .await
        .map_err(tele_invocation)?;
    Ok(match document_id {
        Some(id) => serde_json::json!({"emoji_status": id, "removed": false}),
        None => serde_json::json!({"emoji_status": null, "removed": true}),
    })
}

pub(crate) fn profile_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "profile emoji-status",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "set or clear my emoji status",
            EmojiStatusParams,
            EmojiStatusArgs,
            validate_emoji_status,
            emoji_status_serve_dry_run,
            run_emoji_status,
            crate::commands::serve::schema_placeholder
        ),
        crate::serve_route!(
            "profile get",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "show my profile",
            GetParams,
            GetArgs,
            validate_get,
            get_serve_dry_run,
            run_get,
            crate::commands::serve::schema_placeholder
        ),
        crate::serve_route!(
            "profile photo",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "set or clear my profile photo",
            PhotoParams,
            PhotoArgs,
            validate_photo,
            photo_serve_dry_run,
            run_photo,
            crate::commands::serve::schema_placeholder
        ),
        crate::serve_route!(
            "profile set",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "update my name or bio",
            SetParams,
            SetArgs,
            validate_set,
            set_serve_dry_run,
            run_set,
            crate::commands::serve::schema_placeholder
        ),
    ]
}

crate::serve_runner!(run_get, get_core, GetParams);
crate::serve_runner!(run_set, set_core, SetParams);
crate::serve_runner!(run_photo, photo_core, PhotoParams);
crate::serve_runner!(run_emoji_status, emoji_status_core, EmojiStatusParams);

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn set_requires_at_least_one_flag() {
        let none = SetArgs {
            name: None,
            bio: None,
            photo: None,
            username: None,
        };
        assert!(matches!(validate_set(&none), Err(TeleError::Usage(_))));
        let with_bio = SetArgs {
            name: None,
            bio: Some("b".to_string()),
            photo: None,
            username: None,
        };
        assert!(validate_set(&with_bio).is_ok());
        let with_name = SetArgs {
            name: Some("n".to_string()),
            bio: None,
            photo: None,
            username: None,
        };
        assert!(validate_set(&with_name).is_ok());
    }

    #[test]
    fn split_full_name_preserves_last_name_semantics() {
        assert_eq!(split_full_name("John"), (Some("John".to_string()), None));
        assert_eq!(
            split_full_name("John Smith"),
            (Some("John".to_string()), Some("Smith".to_string()))
        );
        assert_eq!(
            split_full_name("A  B"),
            (Some("A".to_string()), Some("B".to_string()))
        );
        assert_eq!(split_full_name("John "), (Some("John".to_string()), None));
        assert_eq!(
            split_full_name(" Mary Jane Watson "),
            (Some("Mary".to_string()), Some("Jane Watson".to_string()))
        );
    }

    #[test]
    fn validate_set_rejects_oversized_fields() {
        let long_first = SetArgs {
            name: Some("x".repeat(65)),
            bio: None,
            photo: None,
            username: None,
        };
        assert!(matches!(
            validate_set(&long_first),
            Err(TeleError::Usage(_))
        ));
        let long_last = SetArgs {
            name: Some(format!("ok {}", "y".repeat(65))),
            bio: None,
            photo: None,
            username: None,
        };
        assert!(matches!(validate_set(&long_last), Err(TeleError::Usage(_))));
        let long_bio = SetArgs {
            name: None,
            bio: Some("z".repeat(141)),
            photo: None,
            username: None,
        };
        assert!(matches!(validate_set(&long_bio), Err(TeleError::Usage(_))));
        let at_cap = SetArgs {
            name: Some(format!("{} Wat", "a".repeat(60))),
            bio: Some("b".repeat(140)),
            photo: None,
            username: None,
        };
        assert!(validate_set(&at_cap).is_ok());
    }

    #[test]
    fn set_rejects_empty_name() {
        let empty = SetArgs {
            name: Some(String::new()),
            bio: None,
            photo: None,
            username: None,
        };
        assert!(matches!(validate_set(&empty), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_rejects_whitespace_only_name() {
        let blank = SetArgs {
            name: Some("   ".to_string()),
            bio: None,
            photo: None,
            username: None,
        };
        assert!(matches!(validate_set(&blank), Err(TeleError::Usage(_))));
    }

    #[test]
    fn set_accepts_valid_name() {
        let valid = SetArgs {
            name: Some("John Doe".to_string()),
            bio: None,
            photo: None,
            username: None,
        };
        assert!(validate_set(&valid).is_ok());
    }

    #[test]
    fn set_accepts_empty_bio_with_valid_name() {
        let clear_bio = SetArgs {
            name: Some("John".to_string()),
            bio: Some(String::new()),
            photo: None,
            username: None,
        };
        assert!(validate_set(&clear_bio).is_ok());
    }

    #[test]
    fn validate_username_accepts_valid_shapes() {
        for ok in [
            "johny",
            "john_doe",
            "JohnDoe1",
            "a1b2c",
            &"x".repeat(MIN_USERNAME_CHARS),
            &"z".repeat(MAX_USERNAME_CHARS),
        ] {
            assert!(validate_username(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn validate_username_rejects_bad_shapes() {
        let cases = [
            ("", "empty"),
            ("abc", "too short"),
            (&"y".repeat(MAX_USERNAME_CHARS + 1), "too long"),
            ("12345", "digits only, also leading digit"),
            ("1john", "leading digit"),
            ("john_", "trailing underscore"),
            ("john-doe", "dash"),
            ("john doe", "space"),
            ("jöhn", "non-ascii"),
            ("jo.hn", "dot"),
        ];
        for (bad, why) in cases {
            assert!(
                matches!(validate_username(bad), Err(TeleError::Usage(_))),
                "{why}: {bad}"
            );
        }
    }

    #[test]
    fn validate_username_arg_strips_prefixes_and_at() {
        for raw in ["@john_doe", "t.me/john_doe", "https://t.me/john_doe"] {
            assert!(validate_username_arg(Some(raw)).is_ok(), "{raw}");
        }
        assert!(validate_username_arg(Some("REMOVE")).is_ok());
        assert!(validate_username_arg(Some("remove")).is_ok());
        assert!(validate_username_arg(None).is_ok());
    }

    #[test]
    fn set_with_only_username_is_valid() {
        let only_username = SetArgs {
            name: None,
            bio: None,
            photo: None,
            username: Some("@john_doe".to_string()),
        };
        assert!(validate_set(&only_username).is_ok());
        let bad_username = SetArgs {
            name: None,
            bio: None,
            photo: None,
            username: Some("1bad!".to_string()),
        };
        assert!(matches!(
            validate_set(&bad_username),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn validate_emoji_status_matrix() {
        let set_ok = EmojiStatusArgs {
            emoji: Some(5312345678),
            remove: false,
        };
        assert_eq!(validate_emoji_status(&set_ok).unwrap(), Some(5312345678));
        let clear_ok = EmojiStatusArgs {
            emoji: None,
            remove: true,
        };
        assert_eq!(validate_emoji_status(&clear_ok).unwrap(), None);
        let both = EmojiStatusArgs {
            emoji: Some(1),
            remove: true,
        };
        assert!(matches!(
            validate_emoji_status(&both),
            Err(TeleError::Usage(_))
        ));
        let neither = EmojiStatusArgs {
            emoji: None,
            remove: false,
        };
        assert!(matches!(
            validate_emoji_status(&neither),
            Err(TeleError::Usage(_))
        ));
        let negative = EmojiStatusArgs {
            emoji: Some(-1),
            remove: false,
        };
        assert!(matches!(
            validate_emoji_status(&negative),
            Err(TeleError::Usage(_))
        ));
        let zero = EmojiStatusArgs {
            emoji: Some(0),
            remove: false,
        };
        assert!(matches!(
            validate_emoji_status(&zero),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn cli_parses_new_profile_subcommands() {
        let photo_remove = crate::Cli::try_parse_from(["tele", "profile", "photo", "--remove"]);
        match photo_remove {
            Ok(cli) => {
                assert!(matches!(
                    cli.command,
                    crate::Command::Profile(ProfileCmd::Photo(PhotoArgs { remove: true }))
                ));
            }
            Err(e) => panic!("profile photo --remove failed to parse: {e}"),
        }
        let emoji_set = crate::Cli::try_parse_from([
            "tele",
            "profile",
            "emoji-status",
            "--emoji",
            "5312345678",
        ]);
        match emoji_set {
            Ok(cli) => {
                let crate::Command::Profile(ProfileCmd::EmojiStatus(args)) = cli.command else {
                    panic!("expected emoji-status");
                };
                assert_eq!(args.emoji, Some(5312345678));
            }
            Err(e) => panic!("profile emoji-status failed to parse: {e}"),
        }
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

    #[test]
    fn phone_redacted_by_default() {
        assert_eq!(redact_phone(Some("+123456789"), false), None);
        assert_eq!(redact_phone(None, false), None);
    }

    #[test]
    fn phone_shown_when_flag_set() {
        assert_eq!(redact_phone(Some("+123456789"), true), Some("+123456789"));
        assert_eq!(redact_phone(None, true), None);
    }

    #[test]
    fn phone_json_key_is_null_or_string() {
        let row = serde_json::json!({"phone": redact_phone(Some("+123456789"), false)});
        assert!(row["phone"].is_null());
        let row = serde_json::json!({"phone": redact_phone(Some("+123456789"), true)});
        assert_eq!(row["phone"], serde_json::json!("+123456789"));
    }

    fn set_args(photo: Option<&str>) -> SetArgs {
        SetArgs {
            name: None,
            bio: None,
            photo: photo.map(str::to_string),
            username: None,
        }
    }

    #[test]
    fn set_rejects_sensitive_photo_basenames() {
        for bad in [
            "C:/secrets/.env",
            "C:/telecli-data/1.session",
            "2.session-journal",
        ] {
            assert!(
                matches!(validate_set(&set_args(Some(bad))), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn set_rejects_photo_from_app_data_dir() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let dir = crate::config::app_data_dir();
        for bad in ["telecli-profile-test.toml", "telecli-profile-test.env"] {
            let path = dir.join(bad).to_string_lossy().into_owned();
            assert!(
                matches!(
                    validate_set(&set_args(Some(&path))),
                    Err(TeleError::Usage(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn set_accepts_regular_photo_paths() {
        let dir =
            std::env::temp_dir().join(format!("telecli-profile-photo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("photo.jpg");
        std::fs::write(&photo, b"x").unwrap();
        assert!(validate_set(&set_args(Some(&photo.to_string_lossy()))).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plan_for(
        op: &str,
        params: serde_json::Value,
    ) -> Result<crate::commands::serve::Plan, serde_json::Value> {
        let routes = profile_serve_routes();
        let route = routes
            .iter()
            .find(|r| r.op == op)
            .unwrap_or_else(|| panic!("route missing for {op}"));
        (route.planner)(op, params)
    }

    #[test]
    fn serve_routes_declare_lanes_and_timeouts() {
        use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
        let routes = profile_serve_routes();
        let want: Vec<(&str, Lane, Option<std::time::Duration>)> = vec![
            (
                "profile emoji-status",
                Lane::Mutate,
                Some(OP_TIMEOUT_SIMPLE),
            ),
            ("profile get", Lane::Read, Some(OP_TIMEOUT_PAGINATED)),
            ("profile photo", Lane::Mutate, Some(OP_TIMEOUT_SIMPLE)),
            ("profile set", Lane::Mutate, Some(OP_TIMEOUT_SIMPLE)),
        ];
        assert_eq!(routes.len(), want.len());
        for (op, lane, timeout) in want {
            let route = routes
                .iter()
                .find(|r| r.op == op)
                .unwrap_or_else(|| panic!("{op}"));
            assert_eq!(route.lane, lane, "{op}");
            assert_eq!(route.timeout, timeout, "{op}");
        }
    }

    #[test]
    fn serve_wrong_type_param_yields_serve_error() {
        for (op, params, fragment) in [
            (
                "profile get",
                serde_json::json!({"chat": 5}),
                "expected a string",
            ),
            (
                "profile set",
                serde_json::json!({"bio": 7}),
                "expected a string",
            ),
            (
                "profile photo",
                serde_json::json!({"remove": "yes"}),
                "expected a boolean",
            ),
            (
                "profile emoji-status",
                serde_json::json!({"emoji": "big"}),
                "i64",
            ),
        ] {
            let err = plan_for(op, params).unwrap_err();
            assert_eq!(err["type"], "ServeError", "{op}");
            let msg = err["message"].as_str().unwrap();
            assert!(msg.contains(op), "{op}: {msg}");
            assert!(msg.contains(fragment), "{op}: {msg}");
        }
    }

    #[test]
    fn serve_unknown_param_yields_serve_error() {
        let err = plan_for("profile set", serde_json::json!({"nam": "typo"})).unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("unknown field"), "{msg}");
        assert!(msg.contains("nam"), "{msg}");
    }

    #[test]
    fn serve_validation_usage_errors_stay_pure() {
        let err = plan_for("profile set", serde_json::json!({})).unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("--name"));

        let err = plan_for("profile set", serde_json::json!({"username": "1bad!"})).unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for("profile set", serde_json::json!({"bio": "z".repeat(141)})).unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for("profile set", serde_json::json!({"name": ""})).unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for("profile photo", serde_json::json!({})).unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("--remove"));

        let err = plan_for("profile emoji-status", serde_json::json!({})).unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for("profile emoji-status", serde_json::json!({"emoji": -1})).unwrap_err();
        assert_eq!(err["type"], "UsageError");

        let err = plan_for(
            "profile emoji-status",
            serde_json::json!({"emoji": 1, "remove": true}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "UsageError");
    }

    #[test]
    fn serve_rejects_sensitive_photo_paths_through_planner() {
        for bad in [
            serde_json::json!({"photo": "C:/secrets/.env"}),
            serde_json::json!({"photo": "2.session-journal"}),
        ] {
            let err = plan_for("profile set", bad).unwrap_err();
            assert_eq!(err["type"], "UsageError");
        }
    }

    #[test]
    fn serve_dry_run_payloads_match_cli_shapes() {
        let plan = plan_for(
            "profile get",
            serde_json::json!({"chat": "me", "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({"dry_run": true, "chat": "me", "would": "get profile of user me"})
        );

        let plan = plan_for("profile get", serde_json::json!({"dry_run": true})).unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({"dry_run": true, "would": "get own profile"})
        );

        let plan = plan_for(
            "profile set",
            serde_json::json!({"name": "John Doe", "bio": "hi", "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({"dry_run": true, "would": "set profile name, bio"})
        );

        let dir =
            std::env::temp_dir().join(format!("telecli-profile-serve-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("photo.jpg");
        std::fs::write(&photo, b"x").unwrap();
        let plan = plan_for(
            "profile set",
            serde_json::json!({
                "username": "@john_doe",
                "photo": photo.to_string_lossy(),
                "dry_run": true
            }),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({"dry_run": true, "would": "set profile photo, username"})
        );
        let _ = std::fs::remove_dir_all(&dir);

        let plan = plan_for(
            "profile photo",
            serde_json::json!({"remove": true, "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({"dry_run": true, "would": "remove current profile photo"})
        );

        let plan = plan_for(
            "profile emoji-status",
            serde_json::json!({"emoji": 5312345678i64, "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({
                "dry_run": true,
                "would": "set emoji status to emoji document 5312345678"
            })
        );

        let plan = plan_for(
            "profile emoji-status",
            serde_json::json!({"remove": true, "dry_run": true}),
        )
        .unwrap();
        let crate::commands::serve::Plan::DryRun(v) = plan else {
            panic!("expected dry run plan")
        };
        assert_eq!(
            v,
            serde_json::json!({"dry_run": true, "would": "clear emoji status"})
        );
    }

    #[test]
    fn serve_execute_plan_passes_raw_params_through() {
        for (op, raw) in [
            ("profile get", serde_json::json!({"show_phone": true})),
            ("profile get", serde_json::json!({})),
            ("profile set", serde_json::json!({"name": "New Name"})),
            ("profile set", serde_json::json!({"username": "remove"})),
            ("profile photo", serde_json::json!({"remove": true})),
            ("profile emoji-status", serde_json::json!({"emoji": 777i64})),
        ] {
            let plan = plan_for(op, raw.clone()).unwrap();
            match plan {
                crate::commands::serve::Plan::Execute(passed) => {
                    assert_eq!(passed, raw, "{op}")
                }
                other => panic!("expected execute plan for {op}, got {other:?}"),
            }
        }
    }
}
