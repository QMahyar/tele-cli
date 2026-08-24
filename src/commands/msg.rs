use clap::{Args, Subcommand};
use grammers_client::message::InputMessage;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::require_chat_target;
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::fs_util::{path_under_guard, resolve_for_guard};
use crate::output;

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
}

#[derive(Args, Clone)]
pub struct SendArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "message text (mutually exclusive with --file)")]
    text: Option<String>,
    #[arg(
        long,
        help = "send time: Unix timestamp or RFC3339 datetime (must be in the future)"
    )]
    schedule: Option<String>,
    #[arg(
        long = "file",
        value_name = "PATH",
        help = "file path(s) to upload; 2-10 paths send as an album (mutually exclusive with --text)"
    )]
    files: Vec<String>,
    #[arg(long, help = "caption for uploaded file(s) (requires --file)")]
    caption: Option<String>,
    #[arg(long, help = "message ID to reply to")]
    reply: Option<i32>,
    #[arg(
        long,
        help = "forum topic ID to post into (conflicts with --reply)",
        conflicts_with = "reply"
    )]
    topic: Option<i32>,
    #[arg(
        long,
        help = "auto-destruct media after N seconds",
        value_name = "SECS"
    )]
    media_ttl: Option<i32>,
    #[arg(
        long,
        help = "custom thumbnail path for document uploads",
        value_name = "PATH"
    )]
    thumbnail: Option<String>,
    #[arg(
        long,
        help = "upload a remote URL as media (with --kind photo|document)",
        value_name = "URL"
    )]
    url: Option<String>,
    #[arg(
        long,
        help = "media kind for --url: photo or document",
        requires = "url"
    )]
    kind: Option<String>,
    #[arg(
        long,
        help = "re-send media from this chat's message without forward header (requires --copy-id)",
        value_name = "CHAT"
    )]
    copy_from: Option<String>,
    #[arg(
        long,
        help = "message ID whose media is re-sent (requires --copy-from)",
        requires = "copy_from",
        value_name = "ID"
    )]
    copy_id: Option<i32>,
    #[arg(long, default_value_t = true, help = "show link preview")]
    preview: bool,
    #[arg(long, action = clap::ArgAction::SetTrue, help = "disable link preview")]
    no_preview: bool,
    #[arg(long, default_value = "plain", help = "text format: plain or markdown")]
    format: String,
    #[arg(long, help = "send without notification sound")]
    silent: bool,
    #[arg(
        long,
        help = "disallow forwarding and saving of this message (text sends only)"
    )]
    noforwards: bool,
    #[arg(
        long,
        help = "send as a background message (no notification sound even in unmuted chats)"
    )]
    background: bool,
}

#[derive(Args, Clone)]
pub struct EditArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "message ID to edit")]
    id: i32,
    #[arg(long, help = "new message text")]
    text: String,
}

#[derive(Args, Clone)]
pub struct DeleteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "comma-separated message IDs to delete"
    )]
    ids: Vec<i32>,
    #[arg(long, help = "delete all messages in chat")]
    all: bool,
    #[arg(
        long,
        help = "delete only for yourself (no revoke; private chats and basic groups only, not channels)"
    )]
    self_only: bool,
}

#[derive(Args, Clone)]
pub struct ForwardArgs {
    #[arg(long, help = "source chat to forward from")]
    from: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "comma-separated message IDs to forward"
    )]
    ids: Vec<i32>,
    #[arg(long, help = "destination chat to forward to")]
    to: String,
}

#[derive(Args, Clone)]
pub struct PinArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "message ID to pin or unpin")]
    id: Option<i32>,
    #[arg(long, help = "remove pin instead of adding")]
    unpin: bool,
    #[arg(
        long,
        help = "notify chat members when pinning",
        conflicts_with_all = &["unpin", "show", "all"]
    )]
    notify: bool,
    #[arg(
        long,
        help = "show the currently pinned message",
        conflicts_with_all = &["id", "unpin", "all", "notify"]
    )]
    show: bool,
    #[arg(
        long,
        help = "unpin all pinned messages",
        conflicts_with_all = &["id", "unpin", "show", "notify"]
    )]
    all: bool,
}

#[derive(Args, Clone)]
pub struct GetArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        help = "fetch a single message by ID (conflicts with a msg id carried in --chat)"
    )]
    id: Option<i32>,
    #[arg(long, default_value_t = 10, help = "max results to return (1-10000)")]
    limit: u32,
    #[arg(long, help = "fetch messages before this ID")]
    offset_id: Option<i32>,
    #[arg(long, help = "fetch only the most recent message")]
    last: bool,
}

#[derive(Args, Clone)]
pub struct ReadArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "mark as unread instead of read")]
    mark_unread: bool,
    #[arg(
        long,
        help = "clear the mention badge only",
        conflicts_with = "mark_unread"
    )]
    mentions: bool,
}

#[derive(Args, Clone)]
pub struct ReactArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "message ID to react to")]
    id: i32,
    #[arg(long, help = "emoji reaction to add")]
    reaction: Option<String>,
    #[arg(long, help = "remove reaction instead of adding")]
    remove: bool,
}

#[derive(Args, Clone)]
pub struct SearchArgs {
    #[arg(
        long,
        default_value = "",
        help = "target chat: @username, t.me link, numeric ID, +phone, or me (not required with --global)"
    )]
    chat: String,
    #[arg(long, help = "search query text")]
    query: String,
    #[arg(long, default_value_t = 10, help = "max results to return (1-10000)")]
    limit: u32,
    #[arg(
        long,
        help = "search across all dialogs instead of one chat",
        requires = "query"
    )]
    global: bool,
}

#[derive(Args, Clone)]
pub struct DownloadArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "message ID to download media from")]
    id: i32,
    #[arg(long, help = "output directory for downloaded media")]
    dir: String,
    #[arg(long, help = "overwrite existing files")]
    force: bool,
    #[arg(long, help = "streaming chunk size in KB (4-512, multiple of 4)")]
    chunk_size_kb: Option<usize>,
}

#[derive(Args, Clone)]
pub struct VoteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "message ID of the poll")]
    id: i32,
    #[arg(
        long,
        help = "1-based option indexes to vote for, e.g. 1 or 1,3 (multiple only for multi-choice polls)"
    )]
    option: String,
}

#[derive(Args, Clone)]
pub struct TypingArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(
        long,
        help = "chat action: typing | upload-photo | upload-file | cancel (default typing; actions auto-expire)"
    )]
    action: Option<String>,
}

#[derive(Args, Clone)]
pub struct ClickArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, help = "message ID carrying the inline keyboard")]
    id: i32,
    #[arg(
        long,
        value_name = "TEXT",
        help = "inline button text to click",
        conflicts_with = "button_index"
    )]
    button: Option<String>,
    #[arg(
        long,
        value_name = "N",
        help = "1-based inline button position across all rows",
        conflicts_with = "button"
    )]
    button_index: Option<usize>,
    #[arg(
        long,
        help = "reserved for 2FA-protected buttons; not supported at this layer"
    )]
    password: bool,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) text: Option<String>,
    pub(crate) schedule: Option<String>,
    #[serde(default)]
    pub(crate) files: Vec<String>,
    pub(crate) caption: Option<String>,
    pub(crate) reply: Option<i32>,
    pub(crate) topic: Option<i32>,
    pub(crate) media_ttl: Option<i32>,
    pub(crate) thumbnail: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) copy_from: Option<String>,
    pub(crate) copy_id: Option<i32>,
    #[serde(default = "default_true")]
    pub(crate) preview: bool,
    #[serde(default)]
    pub(crate) no_preview: bool,
    #[serde(default = "default_format")]
    pub(crate) format: String,
    #[serde(default)]
    pub(crate) silent: bool,
    #[serde(default)]
    pub(crate) noforwards: bool,
    #[serde(default)]
    pub(crate) background: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&SendArgs> for SendParams {
    fn from(a: &SendArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            text: a.text.clone(),
            schedule: a.schedule.clone(),
            files: a.files.clone(),
            caption: a.caption.clone(),
            reply: a.reply,
            topic: a.topic,
            media_ttl: a.media_ttl,
            thumbnail: a.thumbnail.clone(),
            url: a.url.clone(),
            kind: a.kind.clone(),
            copy_from: a.copy_from.clone(),
            copy_id: a.copy_id,
            preview: a.preview,
            no_preview: a.no_preview,
            format: a.format.clone(),
            silent: a.silent,
            noforwards: a.noforwards,
            background: a.background,
            dry_run: false,
        }
    }
}

impl From<&SendParams> for SendArgs {
    fn from(p: &SendParams) -> Self {
        Self {
            chat: p.chat.clone(),
            text: p.text.clone(),
            schedule: p.schedule.clone(),
            files: p.files.clone(),
            caption: p.caption.clone(),
            reply: p.reply,
            topic: p.topic,
            media_ttl: p.media_ttl,
            thumbnail: p.thumbnail.clone(),
            url: p.url.clone(),
            kind: p.kind.clone(),
            copy_from: p.copy_from.clone(),
            copy_id: p.copy_id,
            preview: p.preview,
            no_preview: p.no_preview,
            format: p.format.clone(),
            silent: p.silent,
            noforwards: p.noforwards,
            background: p.background,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    #[serde(default)]
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&EditArgs> for EditParams {
    fn from(a: &EditArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            id: a.id,
            text: a.text.clone(),
            dry_run: false,
        }
    }
}

impl From<&EditParams> for EditArgs {
    fn from(p: &EditParams) -> Self {
        Self {
            chat: p.chat.clone(),
            id: p.id,
            text: p.text.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) ids: Vec<i32>,
    #[serde(default)]
    pub(crate) all: bool,
    #[serde(default)]
    pub(crate) self_only: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&DeleteArgs> for DeleteParams {
    fn from(a: &DeleteArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            ids: a.ids.clone(),
            all: a.all,
            self_only: a.self_only,
            dry_run: false,
        }
    }
}

impl From<&DeleteParams> for DeleteArgs {
    fn from(p: &DeleteParams) -> Self {
        Self {
            chat: p.chat.clone(),
            ids: p.ids.clone(),
            all: p.all,
            self_only: p.self_only,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForwardParams {
    #[serde(default)]
    pub(crate) from: String,
    #[serde(default)]
    pub(crate) ids: Vec<i32>,
    #[serde(default)]
    pub(crate) to: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ForwardArgs> for ForwardParams {
    fn from(a: &ForwardArgs) -> Self {
        Self {
            from: a.from.clone(),
            ids: a.ids.clone(),
            to: a.to.clone(),
            dry_run: false,
        }
    }
}

impl From<&ForwardParams> for ForwardArgs {
    fn from(p: &ForwardParams) -> Self {
        Self {
            from: p.from.clone(),
            ids: p.ids.clone(),
            to: p.to.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PinParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: Option<i32>,
    #[serde(default)]
    pub(crate) unpin: bool,
    #[serde(default)]
    pub(crate) notify: bool,
    #[serde(default)]
    pub(crate) show: bool,
    #[serde(default)]
    pub(crate) all: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&PinArgs> for PinParams {
    fn from(a: &PinArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            id: a.id,
            unpin: a.unpin,
            notify: a.notify,
            show: a.show,
            all: a.all,
            dry_run: false,
        }
    }
}

impl From<&PinParams> for PinArgs {
    fn from(p: &PinParams) -> Self {
        Self {
            chat: p.chat.clone(),
            id: p.id,
            unpin: p.unpin,
            notify: p.notify,
            show: p.show,
            all: p.all,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) id: Option<i32>,
    #[serde(default = "default_limit")]
    pub(crate) limit: u32,
    pub(crate) offset_id: Option<i32>,
    #[serde(default)]
    pub(crate) last: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&GetArgs> for GetParams {
    fn from(a: &GetArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            id: a.id,
            limit: a.limit,
            offset_id: a.offset_id,
            last: a.last,
            dry_run: false,
        }
    }
}

impl From<&GetParams> for GetArgs {
    fn from(p: &GetParams) -> Self {
        Self {
            chat: p.chat.clone(),
            id: p.id,
            limit: p.limit,
            offset_id: p.offset_id,
            last: p.last,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) mark_unread: bool,
    #[serde(default)]
    pub(crate) mentions: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ReadArgs> for ReadParams {
    fn from(a: &ReadArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            mark_unread: a.mark_unread,
            mentions: a.mentions,
            dry_run: false,
        }
    }
}

impl From<&ReadParams> for ReadArgs {
    fn from(p: &ReadParams) -> Self {
        Self {
            chat: p.chat.clone(),
            mark_unread: p.mark_unread,
            mentions: p.mentions,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReactParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    pub(crate) reaction: Option<String>,
    #[serde(default)]
    pub(crate) remove: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ReactArgs> for ReactParams {
    fn from(a: &ReactArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            id: a.id,
            reaction: a.reaction.clone(),
            remove: a.remove,
            dry_run: false,
        }
    }
}

impl From<&ReactParams> for ReactArgs {
    fn from(p: &ReactParams) -> Self {
        Self {
            chat: p.chat.clone(),
            id: p.id,
            reaction: p.reaction.clone(),
            remove: p.remove,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) query: String,
    #[serde(default = "default_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) global: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&SearchArgs> for SearchParams {
    fn from(a: &SearchArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            query: a.query.clone(),
            limit: a.limit,
            global: a.global,
            dry_run: false,
        }
    }
}

impl From<&SearchParams> for SearchArgs {
    fn from(p: &SearchParams) -> Self {
        Self {
            chat: p.chat.clone(),
            query: p.query.clone(),
            limit: p.limit,
            global: p.global,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DownloadParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    #[serde(default)]
    pub(crate) dir: String,
    #[serde(default)]
    pub(crate) force: bool,
    pub(crate) chunk_size_kb: Option<usize>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&DownloadArgs> for DownloadParams {
    fn from(a: &DownloadArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            id: a.id,
            dir: a.dir.clone(),
            force: a.force,
            chunk_size_kb: a.chunk_size_kb,
            dry_run: false,
        }
    }
}

impl From<&DownloadParams> for DownloadArgs {
    fn from(p: &DownloadParams) -> Self {
        Self {
            chat: p.chat.clone(),
            id: p.id,
            dir: p.dir.clone(),
            force: p.force,
            chunk_size_kb: p.chunk_size_kb,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoteParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    #[serde(default)]
    pub(crate) option: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&VoteArgs> for VoteParams {
    fn from(a: &VoteArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            id: a.id,
            option: a.option.clone(),
            dry_run: false,
        }
    }
}

impl From<&VoteParams> for VoteArgs {
    fn from(p: &VoteParams) -> Self {
        Self {
            chat: p.chat.clone(),
            id: p.id,
            option: p.option.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TypingParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) action: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&TypingArgs> for TypingParams {
    fn from(a: &TypingArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            action: a.action.clone(),
            dry_run: false,
        }
    }
}

impl From<&TypingParams> for TypingArgs {
    fn from(p: &TypingParams) -> Self {
        Self {
            chat: p.chat.clone(),
            action: p.action.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClickParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    pub(crate) button: Option<String>,
    pub(crate) button_index: Option<usize>,
    #[serde(default)]
    pub(crate) password: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&ClickArgs> for ClickParams {
    fn from(a: &ClickArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            id: a.id,
            button: a.button.clone(),
            button_index: a.button_index,
            password: a.password,
            dry_run: false,
        }
    }
}

impl From<&ClickParams> for ClickArgs {
    fn from(p: &ClickParams) -> Self {
        Self {
            chat: p.chat.clone(),
            id: p.id,
            button: p.button.clone(),
            button_index: p.button_index,
            password: p.password,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_format() -> String {
    "plain".to_string()
}

fn default_limit() -> u32 {
    10
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
    }
}

fn effective_preview(args: &SendArgs) -> bool {
    args.preview && !args.no_preview
}

pub(crate) fn validate_send(args: &SendArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
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
    if args.caption.is_some() && args.files.is_empty() {
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
    if !args.files.is_empty() {
        if !effective_preview(args) {
            return Err(TeleError::Usage(
                "--no-preview is not supported with --file".to_string(),
            ));
        }
        for path in &args.files {
            validate_upload_path(path)?;
        }
    }
    if let Some(thumb) = &args.thumbnail {
        validate_upload_path(thumb)?;
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
    if args.format == "markdown" {
        if let Some(text) = &args.text {
            validate_markdown(text)?;
        }
        if let Some(caption) = &args.caption {
            validate_markdown(caption)?;
        }
    }
    Ok(())
}

fn validate_markdown(text: &str) -> TeleResult<()> {
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

fn is_reserved_device_name(stem: &str) -> bool {
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

fn validate_filename(name: &str) -> TeleResult<()> {
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

const MAX_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn check_upload_size(bytes: u64) -> TeleResult<()> {
    if bytes > MAX_UPLOAD_BYTES {
        return Err(TeleError::Usage(format!(
            "refusing to upload file larger than 2 GiB (got {bytes} bytes)"
        )));
    }
    Ok(())
}

pub fn validate_upload_path(path: &str) -> TeleResult<()> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    validate_filename(base)?;
    let app_dir = canonical_guard_path(&crate::config::app_data_dir().to_string_lossy());
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
    let path = std::path::Path::new(path);
    if !path.is_file() {
        return Err(TeleError::Usage(format!("upload file not found: {path:?}")));
    }
    check_upload_size(std::fs::metadata(path)?.len())
}

pub fn is_sensitive_basename(lower: &str) -> bool {
    const SUFFIXES: [&str; 7] = [
        ".session",
        ".session-journal",
        ".pem",
        ".key",
        ".p12",
        ".pfx",
        ".kdbx",
    ];
    const PREFIXES: [&str; 6] = [
        ".env",
        "config.toml",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        "id_dsa",
    ];
    const EXACT: [&str; 3] = [".netrc", ".git-credentials", "credentials"];
    SUFFIXES.iter().any(|s| lower.ends_with(s))
        || PREFIXES.iter().any(|s| lower.starts_with(s))
        || EXACT.contains(&lower)
}

fn validate_download_dir(dir: &str) -> TeleResult<()> {
    let app_dir = canonical_guard_path(&crate::config::app_data_dir().to_string_lossy());
    let sessions_dir = canonical_guard_path(&crate::session::session_dir().to_string_lossy());
    let canonical = canonical_guard_path(dir);
    if path_under_guard(&canonical, &app_dir) || path_under_guard(&canonical, &sessions_dir) {
        return Err(TeleError::Usage(
            "refusing to download into the telecli app data directory".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_download(args: &DownloadArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
    validate_download_dir(&args.dir)?;
    if let Some(kb) = args.chunk_size_kb {
        validate_chunk_size_kb(kb)?;
    }
    Ok(())
}

fn canonical_guard_path(path: &str) -> std::path::PathBuf {
    resolve_for_guard(std::path::Path::new(path))
}

const SCHEDULE_ONLINE_SENTINEL: i32 = 0;

fn parse_schedule(value: Option<&str>) -> TeleResult<Option<i32>> {
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

fn message_random_id() -> i64 {
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
    chrono::DateTime::from_timestamp(ts as i64, 0)
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
    Ok(sent_updates_row(&updates, &message))
}

fn send_dry_run_payload(args: &SendArgs, schedule: Option<u64>) -> serde_json::Value {
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
        "would": would,
    })
}

pub(crate) fn send_serve_dry_run(args: &SendArgs) -> TeleResult<serde_json::Value> {
    let schedule = parse_schedule(args.schedule.as_deref())?.map(|s| s as u64);
    Ok(send_dry_run_payload(args, schedule))
}

async fn send(args: SendArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_send(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
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
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn send_core(
    shares: &crate::client::ServeShares,
    params: SendParams,
) -> TeleResult<serde_json::Value> {
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
            .ok_or_else(|| TeleError::Usage(format!("message {id} not found")))?;
        let media = source_msg
            .media()
            .ok_or_else(|| TeleError::Usage(format!("message {id} has no media to copy")))?;
        let base = match format.as_str() {
            "markdown" => InputMessage::new()
                .copy_media(&media)
                .markdown(caption.unwrap_or_default()),
            _ => InputMessage::new().copy_media(&media),
        };
        let base = base.link_preview(preview);
        let sent = shares
            .client
            .send_message(chat_ref, apply_common(base))
            .await
            .map_err(tele_invocation)?;
        return crate::serialize::message_to_json(&sent);
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
        return crate::serialize::message_to_json(&sent);
    }
    if files.len() > 1 {
        let mut medias: Vec<grammers_client::media::InputMedia> = Vec::new();
        for path in &files {
            let uploaded = shares
                .client
                .upload_file(path)
                .await
                .map_err(upload_error)?;
            let mut media = match looks_like_image(path) {
                true => grammers_client::media::InputMedia::new().photo(uploaded),
                false => grammers_client::media::InputMedia::new().document(uploaded),
            };
            media = match format.as_str() {
                "markdown" => media.markdown(caption.clone().unwrap_or_default()),
                _ => media.caption(caption.clone().unwrap_or_default()),
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
    crate::serialize::message_to_json(&sent)
}

pub(crate) fn validate_edit(args: &EditArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
    if args.text.trim().is_empty() {
        return Err(TeleError::Usage("--text must not be empty".to_string()));
    }
    Ok(())
}

fn edit_dry_run_payload(chat: &str, text: &str, id: i32) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "id": id,
        "chat": chat,
        "text": text,
        "would": format!("edit message {id}"),
    })
}

pub(crate) fn edit_serve_dry_run(args: &EditArgs) -> TeleResult<serde_json::Value> {
    Ok(edit_dry_run_payload(&args.chat, &args.text, args.id))
}

async fn edit(args: EditArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_edit(&args)?;
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
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    shares
        .client
        .edit_message(chat_ref, params.id, InputMessage::new().text(params.text))
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({"id": params.id, "edited": true}))
}

pub(crate) fn validate_delete(args: &DeleteArgs) -> TeleResult<()> {
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
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
        },
    }))
}

async fn delete(args: DeleteArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_delete(&args)?;
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
        let (report, partial) = delete_report(requested, count);
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
    require_chat_target(&args.from, "from")?;
    require_chat_target(&args.to, "to")?;
    reject_deep_link_msg_id(&args.from, "from")?;
    reject_deep_link_msg_id(&args.to, "to")?;
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
                None => serde_json::Value::Null,
            }
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
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
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
    require_chat_target(&args.chat, "chat")?;
    if args.last && args.offset_id.is_some() {
        return Err(TeleError::Usage(
            "--last and --offset-id are mutually exclusive".to_string(),
        ));
    }
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let target = get_target_message_id(&GetParams::from(args))?;
    if target.is_some() && (args.last || args.offset_id.is_some()) {
        return Err(TeleError::Usage(
            "--last/--offset-id are mutually exclusive with a target message (--id or deep-link)"
                .to_string(),
        ));
    }
    Ok(())
}

const DEEP_LINK_MSG_ID_CONSUMERS: &str = "tele msg get";

fn reject_deep_link_msg_id(target: &str, flag: &str) -> TeleResult<()> {
    if entities::parse_target(target)?.msg_id.is_some() {
        return Err(TeleError::Usage(format!(
            "--{flag} \"{target}\" carries a deep-link message id; deep-link message ids are only accepted by: {DEEP_LINK_MSG_ID_CONSUMERS}"
        )));
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
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")
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

pub(crate) fn get_serve_dry_run(args: &GetArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat,
        "id": args.id,
        "limit": args.limit,
        "offset_id": args.offset_id,
        "last": args.last,
        "would": format!("get messages from chat {}", args.chat),
    }))
}

fn message_table_rows(rows: &[serde_json::Value]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| {
            vec![
                r["id"].to_string(),
                r["date"].as_str().unwrap_or_default().to_string(),
                r["sender"]["name"].as_str().unwrap_or_default().to_string(),
                truncate_text(r["text"].as_str().unwrap_or_default(), 80),
            ]
        })
        .collect()
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
            let result = get_core(&guard.shares(), GetParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                output::print_account_table(
                    &name,
                    multi,
                    &["id", "date", "sender", "text"],
                    &message_table_rows(result["messages"].as_array().unwrap_or(&empty)),
                )?;
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

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

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
        ),
    }))
}

async fn read(args: ReadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
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
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
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
        "would": format!("vote on poll {} with option(s) {option_indexes:?}", args.id),
    }))
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
        .ok_or_else(|| TeleError::Usage(format!("message {id} not found")))?;
    let poll = match msg.media() {
        Some(grammers_client::media::Media::Poll(poll)) => poll,
        _ => return Err(TeleError::Usage(format!("message {id} has no poll"))),
    };
    if poll.closed() {
        return Err(TeleError::Usage(format!("poll in message {id} is closed")));
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
        "options": option_indexes,
    }))
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
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
    typing_action(args.action.as_deref())?;
    Ok(())
}

pub(crate) fn validate_search(args: &SearchArgs) -> TeleResult<()> {
    if !args.global {
        require_chat_target(&args.chat, "chat")?;
    }
    if !args.chat.trim().is_empty() {
        reject_deep_link_msg_id(&args.chat, "chat")?;
    }
    crate::commands::validate_limit(args.limit, 10_000, "limit").map(|_| ())
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
        "would": format!("send {action_name} chat action to {}", args.chat),
    }))
}

async fn typing(args: TypingArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_typing(&args)?;
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
}

#[derive(Debug)]
struct LocatedButton {
    position: usize,
    text: String,
    callback_data: Vec<u8>,
}

fn button_text(button: &serde_json::Value) -> Option<&str> {
    button.get("text").and_then(|v| v.as_str())
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
    };
    match matches.len() {
        0 => Err(TeleError::Usage(match selector {
            ButtonSelector::Index(n) => {
                format!("no button at position {n} (markup has {total} button(s))")
            }
            ButtonSelector::Text(text) => {
                format!("no button named {text:?} in this message's inline keyboard")
            }
        })),
        1 => {
            let (position, button) = matches[0];
            let text = button_text(button).unwrap_or_default().to_string();
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
                callback_data,
            })
        }
        _ => {
            let positions: Vec<usize> = matches.iter().map(|(p, _)| *p).collect();
            let label = match selector {
                ButtonSelector::Text(text) => format!("{text:?}"),
                ButtonSelector::Index(_) => String::new(),
            };
            Err(TeleError::Usage(format!(
                "button {label} matches multiple buttons at positions {positions:?}; use --button-index"
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
    require_chat_target(&args.chat, "chat")?;
    reject_deep_link_msg_id(&args.chat, "chat")?;
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
    match (&args.button, args.button_index) {
        (None, None) => Err(TeleError::Usage(
            "--button TEXT or --button-index N required".to_string(),
        )),
        (Some(_), Some(_)) => Err(TeleError::Usage(
            "--button and --button-index are mutually exclusive".to_string(),
        )),
        (_, Some(0)) => Err(TeleError::Usage("--button-index is 1-based".to_string())),
        _ => Ok(()),
    }
}

fn click_selector(args: &ClickArgs) -> ButtonSelector {
    if let Some(text) = &args.button {
        ButtonSelector::Text(text.clone())
    } else {
        ButtonSelector::Index(args.button_index.unwrap_or_default())
    }
}

fn click_selector_label(selector: &ButtonSelector) -> String {
    match selector {
        ButtonSelector::Text(t) => format!("button {t:?}"),
        ButtonSelector::Index(n) => format!("button #{n}"),
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
        "would": format!("click {selector_label} on message {}", args.id),
    }))
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
        .ok_or_else(|| TeleError::Usage(format!("message {id} not found")))?;
    let markup = msg
        .reply_markup()
        .ok_or_else(|| TeleError::Usage(format!("message {id} has no reply markup")))?;
    let markup_json = crate::serialize::reply_markup_to_json(&markup);
    let located = locate_button(&markup_json, &selector)?;
    if button_requires_password(&markup, &located.text, &located.callback_data) {
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
            data: Some(located.callback_data.clone()),
            password: None,
        })
        .await
        .map_err(tele_invocation)?;
    let mut row = serde_json::json!({
        "id": id,
        "clicked": true,
        "button": located.text,
        "position": located.position,
    });
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

pub(crate) fn search_serve_dry_run(args: &SearchArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": if args.global { serde_json::Value::Null } else { serde_json::json!(args.chat) },
        "global": args.global,
        "query": args.query,
        "limit": args.limit,
        "would": if args.global {
            format!("search all dialogs for \"{}\"", args.query)
        } else {
            format!("search messages in chat {}", args.chat)
        },
    }))
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
                output::print_account_table(
                    &name,
                    multi,
                    &["id", "date", "sender", "text"],
                    &message_table_rows(result["messages"].as_array().unwrap_or(&empty)),
                )?;
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
    shares.rate_limiter.acquire().await;
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut served = 0usize;
    let limit = params.limit as usize;
    if params.global {
        let mut iter = shares
            .client
            .search_all_messages()
            .query(&params.query)
            .limit(limit);
        while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
            served += 1;
            shares.rate_limiter.acquire_for_items(served).await;
            push_message_row(&mut rows, &msg)?;
        }
    } else {
        let chat =
            entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
        let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
        let mut iter = shares
            .client
            .search_messages(chat_ref)
            .query(&params.query)
            .limit(limit);
        while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
            served += 1;
            shares.rate_limiter.acquire_for_items(served).await;
            push_message_row(&mut rows, &msg)?;
        }
    }
    Ok(serde_json::json!({"messages": rows}))
}

pub(crate) fn download_serve_dry_run(args: &DownloadArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "id": args.id,
        "would": format!("download message {}", args.id)
    }))
}

async fn download(args: DownloadArgs, flags: &GlobalFlags) -> TeleResult<i32> {
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
            client::authorize(&guard.client).await?;
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
    tokio::task::spawn_blocking({
        let out_dir = out_dir.clone();
        move || std::fs::create_dir_all(&out_dir)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    validate_download_dir(&out_dir)?;
    let path = std::path::Path::new(&out_dir).join(name);
    if !params.force {
        refuse_existing_download_target(&path)?;
    }
    let temp = download_temp_path(&path);
    tokio::task::spawn_blocking({
        let temp = temp.clone();
        move || create_download_temp(&temp)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    if msg.media().is_none() {
        return Err(TeleError::Usage("message has no media".to_string()));
    }
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
    commit_download(&temp, &path)?;
    let bytes = tokio::task::spawn_blocking({
        let path = path.clone();
        move || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))?;
    Ok(serde_json::json!({"path": path.to_string_lossy(), "bytes": bytes}))
}

fn validate_chunk_size_kb(kb: usize) -> TeleResult<()> {
    if !(4..=512).contains(&kb) || !kb.is_multiple_of(4) {
        return Err(TeleError::Usage(
            "--chunk-size-kb must be 4-512 and a multiple of 4".to_string(),
        ));
    }
    Ok(())
}

fn download_name(msg: &grammers_client::message::Message) -> String {
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

fn sanitize_download_name(name: &str) -> String {
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
    } else if is_reserved_device_name(trimmed.split('.').next().unwrap_or(trimmed)) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn download_temp_path(final_path: &std::path::Path) -> std::path::PathBuf {
    let name = final_path.file_name().unwrap_or_default().to_string_lossy();
    final_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".{name}.part-{}", std::process::id()))
}

fn refuse_existing_download_target(path: &std::path::Path) -> TeleResult<()> {
    if path.exists() {
        return Err(TeleError::Usage(format!(
            "download target exists: {}",
            path.display()
        )));
    }
    Ok(())
}

fn create_download_temp(temp: &std::path::Path) -> TeleResult<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)
        .map(|_| ())
        .map_err(|e| TeleError::Other(format!("cannot create temp file {}: {e}", temp.display())))
}

fn commit_download(temp: &std::path::Path, final_path: &std::path::Path) -> TeleResult<()> {
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

fn looks_like_image(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_session::types::PeerKind;

    #[test]
    fn upload_flood_wait_carries_rpc_keys_like_send_path() {
        let rpc = grammers_client::sender::RpcError {
            code: 420,
            name: "FLOOD_WAIT".to_string(),
            value: Some(17),
            caused_by: None,
        };
        let e = std::io::Error::other(grammers_client::InvocationError::Rpc(rpc));
        let err = upload_error(e);
        assert!(matches!(err, TeleError::Rpc(_, 420, _, Some(17))));
        assert_eq!(err.message(), "rpc error 420: FLOOD_WAIT (value: 17)");
    }

    #[test]
    fn upload_non_flood_error_maps_to_other() {
        let e = std::io::Error::other(grammers_client::InvocationError::Dropped);
        assert!(matches!(upload_error(e), TeleError::Other(_)));
    }

    fn send_args(format: &str) -> SendArgs {
        SendArgs {
            chat: "me".to_string(),
            text: Some("hi".to_string()),
            schedule: None,
            files: vec![],
            media_ttl: None,
            thumbnail: None,
            url: None,
            kind: None,
            copy_from: None,
            copy_id: None,
            caption: None,
            reply: None,
            topic: None,
            preview: true,
            no_preview: false,
            format: format.to_string(),
            silent: false,
            noforwards: false,
            background: false,
        }
    }

    #[test]
    fn send_params_deserialize_with_cli_defaults() {
        let p: SendParams = serde_json::from_value(serde_json::json!({"chat": "@game"})).unwrap();
        assert_eq!(p.chat, "@game");
        assert!(p.text.is_none());
        assert!(p.files.is_empty());
        assert!(p.preview);
        assert!(!p.no_preview);
        assert_eq!(p.format, "plain");
        assert!(!p.silent);
        assert!(!p.noforwards);
        assert!(!p.background);
        assert!(!p.dry_run);
        let args = SendArgs::from(&p);
        assert_eq!(args.chat, "@game");
        assert_eq!(args.format, "plain");
    }

    #[test]
    fn send_params_roundtrip_preserves_args_fields() {
        let base = send_args("markdown");
        let params = SendParams::from(&base);
        let back = SendArgs::from(&params);
        assert_eq!(back.chat, base.chat);
        assert_eq!(back.text, base.text);
        assert_eq!(back.format, base.format);
        assert_eq!(back.preview, base.preview);
        assert_eq!(back.no_preview, base.no_preview);
        let mut edited = base.clone();
        edited.reply = Some(3);
        edited.topic = Some(9);
        edited.silent = true;
        let roundtrip = SendArgs::from(&SendParams::from(&edited));
        assert_eq!(roundtrip.reply, Some(3));
        assert_eq!(roundtrip.topic, Some(9));
        assert!(roundtrip.silent);
    }

    fn upload_fixture(tag: &str, names: &[&str]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("telecli-msg-upload-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        dir
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("telecli-msg-{tag}-{}", std::process::id()))
    }

    #[test]
    fn upload_rejects_sensitive_basenames() {
        assert!(matches!(
            validate_upload_path("C:/secrets/.env"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("C:/telecli-data/1.session"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("2.session-journal"),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn check_upload_size_accepts_boundary() {
        assert!(check_upload_size(MAX_UPLOAD_BYTES).is_ok());
    }

    #[test]
    fn check_upload_size_rejects_over_cap() {
        let err = check_upload_size(MAX_UPLOAD_BYTES + 1).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("2 GiB"));
    }

    #[test]
    fn validate_upload_path_rejects_config_toml_basename() {
        let dir = temp_path("uploadcfg");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["config.toml", "config.toml.tmp-123", "CONFIG.TOML"] {
            let path = dir.join(name);
            std::fs::write(&path, b"x").unwrap();
            let err = validate_upload_path(path.to_str().unwrap()).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "{name}: {err:?}");
            assert!(err.message().contains("sensitive"), "{name}");
        }
        let ok_path = dir.join("notes.txt");
        std::fs::write(&ok_path, b"x").unwrap();
        validate_upload_path(ok_path.to_str().unwrap()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn is_sensitive_basename_covers_private_key_families() {
        for name in [
            "id_rsa",
            "id_rsa.old",
            "ID_ED25519",
            "id_ecdsa",
            "id_dsa",
            "server.pem",
            "CERT.KEY",
            "keystore.p12",
            "backup.pfx",
            "vault.kdbx",
            ".netrc",
            ".git-credentials",
            ".env.local",
            "work.session",
            "work.session-journal",
        ] {
            assert!(
                is_sensitive_basename(&name.to_lowercase()),
                "{name} must be blocked"
            );
        }
        for name in ["notes.txt", "report.pdf", "archive.tar.gz"] {
            assert!(
                !is_sensitive_basename(&name.to_lowercase()),
                "{name} must be allowed"
            );
        }
    }

    #[test]
    fn validate_upload_path_rejects_aws_credentials_basename() {
        let dir = temp_path("uploadaws");
        let aws = dir.join(".aws");
        std::fs::create_dir_all(&aws).unwrap();
        let path = aws.join("credentials");
        std::fs::write(&path, b"x").unwrap();
        let err = validate_upload_path(path.to_str().unwrap()).unwrap_err();
        assert!(err.message().contains("sensitive"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upload_allows_regular_files() {
        let dir = upload_fixture("regular", &["report.pdf"]);
        let file = dir.join("report.pdf");
        assert!(validate_upload_path(&file.to_string_lossy()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_rejects_unknown_format() {
        assert!(matches!(
            validate_send(&send_args("html")),
            Err(TeleError::Usage(_))
        ));
        assert!(validate_send(&send_args("plain")).is_ok());
        assert!(validate_send(&send_args("markdown")).is_ok());
    }

    #[test]
    fn send_rejects_empty_text() {
        let mut args = send_args("plain");
        args.text = Some("".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_rejects_whitespace_text() {
        let mut args = send_args("plain");
        args.text = Some("   \t ".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_allows_whitespace_inside_text() {
        let mut args = send_args("plain");
        args.text = Some("  hello world  ".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn edit_rejects_empty_text() {
        let args = EditArgs {
            chat: "me".to_string(),
            id: 5,
            text: "".to_string(),
        };
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn edit_rejects_whitespace_text() {
        let args = EditArgs {
            chat: "me".to_string(),
            id: 5,
            text: "   \t ".to_string(),
        };
        assert!(matches!(validate_edit(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn edit_allows_normal_text() {
        let args = EditArgs {
            chat: "me".to_string(),
            id: 5,
            text: "hello".to_string(),
        };
        assert!(validate_edit(&args).is_ok());
    }

    #[test]
    fn send_rejects_text_with_file() {
        let mut args = send_args("plain");
        args.files = vec!["C:/tmp/a.pdf".to_string()];
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_rejects_caption_without_file() {
        let mut args = send_args("plain");
        args.caption = Some("cap".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn send_accepts_file_with_caption() {
        let dir = upload_fixture("caption", &["a.pdf"]);
        let mut args = send_args("plain");
        args.text = None;
        args.files = vec![dir.join("a.pdf").to_string_lossy().into_owned()];
        args.caption = Some("cap".to_string());
        assert!(validate_send(&args).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_rejects_non_numeric_mention_ids() {
        let mut args = send_args("markdown");
        for bad in [
            "[x](tg://user?id=abc)",
            "[x](tg://user?id=12abc)",
            "[x](tg://user?id=)",
            "plain text with tg://user?id=abc",
            "<tg://user?id=-1>",
        ] {
            args.text = Some(bad.to_string());
            assert!(
                matches!(validate_send(&args), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn markdown_accepts_numeric_mention_ids() {
        let mut args = send_args("markdown");
        for good in [
            "[x](tg://user?id=12345678)",
            "<tg://user?id=999>",
            "[a](https://example.com) **bold**",
            "plain text",
        ] {
            args.text = Some(good.to_string());
            assert!(validate_send(&args).is_ok(), "{good}");
        }
    }

    #[test]
    fn markdown_accepts_url_containing_tg_user_substring() {
        assert!(validate_markdown("[x](https://example.com/tg://user?id=abc)").is_ok());
    }

    #[test]
    fn markdown_accepts_https_tg_substring() {
        assert!(validate_markdown("https://tg://user?id=abc").is_ok());
    }

    #[test]
    fn markdown_rejects_genuine_invalid_mention() {
        for bad in [
            "[x](tg://user?id=abc)",
            "<tg://user?id=abc>",
            "plain text with tg://user?id=abc",
            "[x](tg://user?id=12abc)",
        ] {
            assert!(
                matches!(validate_markdown(bad), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn markdown_accepts_genuine_valid_mention() {
        for good in ["[x](tg://user?id=12345678)", "<tg://user?id=999>"] {
            assert!(validate_markdown(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn upload_rejects_sensitive_basenames_case_insensitive() {
        assert!(matches!(
            validate_upload_path("C:/secrets/.ENV"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("C:/telecli-data/1.SESSION"),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_upload_path("2.Session-Journal"),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn schedule_absent_is_none() {
        assert_eq!(parse_schedule(None).unwrap(), None);
    }

    #[test]
    fn schedule_rejects_past_timestamps() {
        for v in ["0", "-5", "1000000", "1970-01-01T00:00:01Z"] {
            assert!(
                matches!(parse_schedule(Some(v)), Err(TeleError::Usage(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn schedule_rejects_beyond_i32_range() {
        for v in [
            "2147483648",
            "9999999999",
            "2038-01-19T03:14:08Z",
            "2100-01-01T00:00:00Z",
        ] {
            assert!(
                matches!(parse_schedule(Some(v)), Err(TeleError::Usage(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn schedule_rejects_garbage() {
        assert!(matches!(
            parse_schedule(Some("not a date")),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn schedule_accepts_future_values() {
        let future_ts = chrono::Utc::now().timestamp() + 3600;
        let ts = parse_schedule(Some(&future_ts.to_string()))
            .unwrap()
            .unwrap();
        assert_eq!(ts, future_ts as i32);
        let future_rfc = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        assert!(parse_schedule(Some(&future_rfc)).unwrap().is_some());
    }

    #[test]
    fn delete_batches_ids_by_100() {
        let ids: Vec<i32> = (1..=250).collect();
        let parts = batches(&ids);
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts.iter().map(|b| b.len()).collect::<Vec<_>>(),
            vec![100, 100, 50]
        );
        let exactly_100: Vec<i32> = (1..=100).collect();
        assert_eq!(batches(&exactly_100).len(), 1);
        assert!(batches(&[]).is_empty());
    }

    #[test]
    fn forward_batch_indexing_does_not_panic_on_short_chunk() {
        let chunk: [i32; 3] = [1, 2, 3];
        let mut forwarded: Vec<serde_json::Value> = Vec::new();
        let mut dropped: Vec<i32> = Vec::new();
        let results = vec![None; 5];
        push_forward_results(&mut forwarded, &mut dropped, &chunk, results).unwrap();
        assert_eq!(dropped, vec![1, 2, 3]);
        assert!(forwarded.is_empty());
    }

    #[test]
    fn delete_requires_ids_unless_all() {
        let args = DeleteArgs {
            chat: "me".to_string(),
            ids: Vec::new(),
            all: false,
            self_only: false,
        };
        assert!(matches!(validate_delete(&args), Err(TeleError::Usage(_))));
        let mut with_all = args;
        with_all.all = true;
        assert!(validate_delete(&with_all).is_ok());
        let with_ids = DeleteArgs {
            chat: "me".to_string(),
            ids: vec![1, 2],
            all: false,
            self_only: false,
        };
        assert!(validate_delete(&with_ids).is_ok());
    }

    #[test]
    fn delete_rejects_all_with_ids() {
        let args = DeleteArgs {
            chat: "me".to_string(),
            ids: vec![1, 2],
            all: true,
            self_only: false,
        };
        assert!(matches!(validate_delete(&args), Err(TeleError::Usage(_))));
    }

    #[test]
    fn delete_report_flags_partial_deletions() {
        let (report, partial) = delete_report(3, 3);
        assert_eq!(report["requested"], serde_json::json!(3));
        assert_eq!(report["deleted"], serde_json::json!(3));
        assert!(!partial);
        assert!(report.get("partial").is_none());
        let (report, partial) = delete_report(3, 1);
        assert_eq!(report["requested"], serde_json::json!(3));
        assert_eq!(report["deleted"], serde_json::json!(1));
        assert!(partial);
        assert_eq!(report["partial"], serde_json::json!(true));
        let (report, partial) = delete_report(3, 0);
        assert_eq!(report["deleted"], serde_json::json!(0));
        assert!(partial);
        assert_eq!(report["partial"], serde_json::json!(true));
        let (_, partial) = delete_report(0, 0);
        assert!(!partial);
    }

    #[test]
    fn validate_delete_rejects_self_only_with_all() {
        let args = DeleteArgs {
            chat: "me".to_string(),
            ids: Vec::new(),
            all: true,
            self_only: true,
        };
        assert!(matches!(validate_delete(&args), Err(TeleError::Usage(_))));
        let with_ids = DeleteArgs {
            chat: "me".to_string(),
            ids: vec![1],
            all: false,
            self_only: true,
        };
        assert!(validate_delete(&with_ids).is_ok());
    }

    #[test]
    fn self_only_supported_kinds() {
        assert!(self_only_supported(PeerKind::User));
        assert!(self_only_supported(PeerKind::Chat));
        assert!(!self_only_supported(PeerKind::Channel));
    }

    #[test]
    fn forward_report_tracks_dropped_and_failed() {
        let forwarded = vec![serde_json::json!({"id": 1})];
        let (report, warn) = forward_report(4, &forwarded, &[2], &[3, 4]);
        assert_eq!(report["requested"], serde_json::json!(4));
        assert_eq!(report["forwarded"], serde_json::json!([{"id": 1}]));
        assert_eq!(report["dropped"]["count"], serde_json::json!(1));
        assert_eq!(report["dropped"]["ids"], serde_json::json!([2]));
        assert_eq!(report["failed"]["count"], serde_json::json!(2));
        assert_eq!(report["failed"]["ids"], serde_json::json!([3, 4]));
        assert!(!warn);
    }

    #[test]
    fn forward_report_warns_when_nothing_confirmed() {
        let (report, warn) = forward_report(2, &[], &[1, 2], &[]);
        assert!(warn);
        assert!(report.get("dropped").is_some());
        assert!(report.get("failed").is_none());
        assert_eq!(report["partial"], serde_json::json!(true));
        let (report, warn) = forward_report(1, &[serde_json::json!({"id": 9})], &[], &[]);
        assert!(!warn);
        assert!(report.get("dropped").is_none());
        assert!(report.get("failed").is_none());
        assert!(report.get("partial").is_none());
    }

    #[test]
    fn forward_report_marks_partial_when_all_chunks_fail() {
        let (value, should_warn) = forward_report(3, &[], &[], &[1, 2, 3]);
        assert_eq!(value["partial"], serde_json::json!(true));
        assert_eq!(value["failed"]["count"], serde_json::json!(3));
        assert!(should_warn);
    }

    #[test]
    fn forward_report_marks_partial_when_some_dropped() {
        let (value, _) = forward_report(2, &[serde_json::json!({"id": 1})], &[2], &[]);
        assert_eq!(value["partial"], serde_json::json!(true));
    }

    #[test]
    fn forward_report_omits_partial_when_all_forwarded() {
        let (value, _) = forward_report(1, &[serde_json::json!({"id": 1})], &[], &[]);
        assert!(value.get("partial").is_none(), "value: {value}");
    }

    #[test]
    fn forward_requires_ids() {
        let args = ForwardArgs {
            from: "a".to_string(),
            ids: Vec::new(),
            to: "b".to_string(),
        };
        assert!(matches!(validate_forward(&args), Err(TeleError::Usage(_))));
        let mut with_ids = args;
        with_ids.ids = vec![3];
        assert!(validate_forward(&with_ids).is_ok());
    }

    #[test]
    fn get_rejects_last_with_offset_id() {
        let args = GetArgs {
            chat: "me".to_string(),
            id: None,
            limit: 10,
            offset_id: Some(5),
            last: true,
        };
        assert!(matches!(validate_get(&args), Err(TeleError::Usage(_))));
        let no_offset = GetArgs {
            chat: "me".to_string(),
            id: None,
            limit: 10,
            offset_id: None,
            last: true,
        };
        assert!(validate_get(&no_offset).is_ok());
        let no_last = GetArgs {
            chat: "me".to_string(),
            id: None,
            limit: 10,
            offset_id: Some(5),
            last: false,
        };
        assert!(validate_get(&no_last).is_ok());
    }

    fn get_params(chat: &str, id: Option<i32>) -> GetParams {
        GetParams {
            chat: chat.to_string(),
            id,
            limit: 10,
            offset_id: None,
            last: false,
            dry_run: false,
        }
    }

    #[test]
    fn get_target_id_comes_from_deep_link_when_no_explicit_id() {
        for (chat, expected) in [
            ("t.me/durov/42", 42),
            ("https://t.me/durov/42", 42),
            ("telegram.me/durov/9", 9),
            ("t.me/c/1234567890/42", 42),
            ("t.me/me/5", 5),
            ("t.me/durov/042/", 42),
            ("t.me/durov/7?single#f", 7),
        ] {
            let target = get_target_message_id(&get_params(chat, None))
                .unwrap_or_else(|e| panic!("{chat}: {e}"));
            assert_eq!(target, Some(expected), "{chat}");
        }
    }

    #[test]
    fn get_target_id_uses_explicit_id_for_plain_targets() {
        for chat in ["@durov", "durov", "-1001234567890", "+15551234567", "me"] {
            let target = get_target_message_id(&get_params(chat, Some(11)))
                .unwrap_or_else(|e| panic!("{chat}: {e}"));
            assert_eq!(target, Some(11), "{chat}");
            assert_eq!(
                get_target_message_id(&get_params(chat, None)).unwrap(),
                None
            );
        }
    }

    #[test]
    fn get_target_id_conflicts_when_link_carries_and_explicit_id_given() {
        let err = get_target_message_id(&get_params("https://t.me/durov/42", Some(7))).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        let msg = err.message();
        assert!(msg.contains("--id 7"), "msg: {msg}");
        assert!(msg.contains("42"), "msg: {msg}");
        assert!(msg.contains("t.me/durov/42"), "msg: {msg}");
        let c_form =
            get_target_message_id(&get_params("t.me/c/1234567890/42", Some(7))).unwrap_err();
        assert!(matches!(c_form, TeleError::Usage(_)));
    }

    #[test]
    fn validate_get_accepts_deep_link_without_explicit_id() {
        let args = GetArgs {
            chat: "t.me/durov/42".to_string(),
            id: None,
            limit: 10,
            offset_id: None,
            last: false,
        };
        assert!(validate_get(&args).is_ok());
        let c_form = GetArgs {
            chat: "https://t.me/c/1234567890/8".to_string(),
            id: None,
            limit: 10,
            offset_id: None,
            last: false,
        };
        assert!(validate_get(&c_form).is_ok());
    }

    #[test]
    fn validate_get_rejects_deep_link_plus_explicit_id() {
        let args = GetArgs {
            chat: "t.me/durov/42".to_string(),
            id: Some(42),
            limit: 10,
            offset_id: None,
            last: false,
        };
        let err = validate_get(&args).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("conflicts"));
    }

    #[test]
    fn validate_get_rejects_target_message_with_listing_flags() {
        for (id, offset_id, last) in [
            (None, None, true),
            (None, Some(5), false),
            (Some(3), None, true),
            (Some(3), Some(5), false),
        ] {
            let args = GetArgs {
                chat: "t.me/durov/42".to_string(),
                id,
                limit: 10,
                offset_id,
                last,
            };
            assert!(
                matches!(validate_get(&args), Err(TeleError::Usage(_))),
                "id={id:?} offset={offset_id:?} last={last}"
            );
        }
        let explicit_ok = GetArgs {
            chat: "@durov".to_string(),
            id: Some(3),
            limit: 10,
            offset_id: None,
            last: false,
        };
        assert!(validate_get(&explicit_ok).is_ok());
    }

    #[test]
    fn get_params_missing_id_deserializes_to_none() {
        let p: GetParams = serde_json::from_value(serde_json::json!({"chat": "@durov"})).unwrap();
        assert_eq!(p.chat, "@durov");
        assert_eq!(p.id, None);
        assert_eq!(p.limit, 10);
        assert!(!p.last);
        assert!(!p.dry_run);
    }

    #[test]
    fn non_get_commands_reject_deep_link_message_ids() {
        let link = "https://t.me/durov/42";
        let plain = "t.me/durov";

        let mut send = send_args("plain");
        send.chat = link.to_string();
        let err = validate_send(&send).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("tele msg get"), "msg: {err}");
        send.chat = plain.to_string();
        assert!(validate_send(&send).is_ok());

        let edit = EditArgs {
            chat: link.to_string(),
            id: 1,
            text: "x".to_string(),
        };
        assert!(matches!(validate_edit(&edit), Err(TeleError::Usage(_))));

        let delete = DeleteArgs {
            chat: link.to_string(),
            ids: vec![1],
            all: false,
            self_only: false,
        };
        assert!(matches!(validate_delete(&delete), Err(TeleError::Usage(_))));

        let fwd = ForwardArgs {
            from: link.to_string(),
            ids: vec![1],
            to: "b".to_string(),
        };
        assert!(matches!(validate_forward(&fwd), Err(TeleError::Usage(_))));
        let fwd_to = ForwardArgs {
            from: "a".to_string(),
            ids: vec![1],
            to: link.to_string(),
        };
        assert!(matches!(
            validate_forward(&fwd_to),
            Err(TeleError::Usage(_))
        ));

        let react = ReactArgs {
            chat: link.to_string(),
            id: 1,
            reaction: Some("+1".to_string()),
            remove: false,
        };
        let err = validate_react(&react).unwrap_err();
        assert!(err.message().contains("tele msg get"), "msg: {err}");

        let vote = VoteArgs {
            chat: link.to_string(),
            id: 4,
            option: "1".to_string(),
        };
        assert!(matches!(validate_vote(&vote), Err(TeleError::Usage(_))));

        let download = DownloadArgs {
            chat: link.to_string(),
            id: 4,
            dir: std::env::temp_dir().to_string_lossy().to_string(),
            force: false,
            chunk_size_kb: None,
        };
        assert!(matches!(
            validate_download(&download),
            Err(TeleError::Usage(_))
        ));

        let read = ReadArgs {
            chat: link.to_string(),
            mark_unread: false,
            mentions: false,
        };
        assert!(matches!(validate_read(&read), Err(TeleError::Usage(_))));

        let pin = PinArgs {
            chat: link.to_string(),
            id: Some(4),
            unpin: false,
            notify: false,
            show: false,
            all: false,
        };
        assert!(matches!(validate_pin(&pin), Err(TeleError::Usage(_))));

        let typing = TypingArgs {
            chat: link.to_string(),
            action: None,
        };
        assert!(matches!(validate_typing(&typing), Err(TeleError::Usage(_))));

        let click = ClickArgs {
            chat: link.to_string(),
            id: 4,
            button: Some("ok".to_string()),
            button_index: None,
            password: false,
        };
        assert!(matches!(validate_click(&click), Err(TeleError::Usage(_))));

        let search = SearchArgs {
            chat: link.to_string(),
            query: "q".to_string(),
            limit: 10,
            global: false,
        };
        assert!(matches!(validate_search(&search), Err(TeleError::Usage(_))));

        let search_global = SearchArgs {
            chat: String::new(),
            query: "q".to_string(),
            limit: 10,
            global: true,
        };
        assert!(validate_search(&search_global).is_ok());
    }

    #[test]
    fn msg_validators_reject_empty_or_whitespace_chat() {
        for chat in ["", "   ", "\t"] {
            let mut send = send_args("plain");
            send.chat = chat.to_string();
            assert!(
                matches!(validate_send(&send), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let edit = EditArgs {
                chat: chat.to_string(),
                id: 1,
                text: "x".to_string(),
            };
            assert!(
                matches!(validate_edit(&edit), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let delete = DeleteArgs {
                chat: chat.to_string(),
                ids: vec![1],
                all: false,
                self_only: false,
            };
            assert!(
                matches!(validate_delete(&delete), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let fwd_from = ForwardArgs {
                from: chat.to_string(),
                ids: vec![1],
                to: "b".to_string(),
            };
            assert!(
                matches!(validate_forward(&fwd_from), Err(TeleError::Usage(_))),
                "{chat:?}"
            );
            let fwd_to = ForwardArgs {
                from: "a".to_string(),
                ids: vec![1],
                to: chat.to_string(),
            };
            assert!(
                matches!(validate_forward(&fwd_to), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let get = GetArgs {
                chat: chat.to_string(),
                id: None,
                limit: 10,
                offset_id: None,
                last: false,
            };
            assert!(
                matches!(validate_get(&get), Err(TeleError::Usage(_))),
                "{chat:?}"
            );

            let react = ReactArgs {
                chat: chat.to_string(),
                id: 1,
                reaction: Some("+1".to_string()),
                remove: false,
            };
            assert!(
                matches!(validate_react(&react), Err(TeleError::Usage(_))),
                "{chat:?}"
            );
        }
    }

    #[tokio::test]
    async fn pin_read_search_download_reject_empty_chat_before_connect() {
        let err = pin(
            PinArgs {
                chat: String::new(),
                id: Some(1),
                unpin: false,
                notify: false,
                show: false,
                all: false,
            },
            &dryrun_flags("msg pin", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));

        let err = read(
            ReadArgs {
                chat: "   ".to_string(),
                mark_unread: false,
                mentions: false,
            },
            &dryrun_flags("msg read", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));

        let err = search(
            SearchArgs {
                chat: String::new(),
                query: "x".to_string(),
                limit: 10,
                global: false,
            },
            &dryrun_flags("msg search", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));

        let out =
            std::env::temp_dir().join(format!("telecli-msg-emptychat-{}", std::process::id()));
        let err = download(
            DownloadArgs {
                chat: "\t".to_string(),
                id: 1,
                dir: out.to_string_lossy().into_owned(),
                force: false,
                chunk_size_kb: None,
            },
            &dryrun_flags("msg download", true),
        )
        .await;
        assert!(matches!(err, Err(TeleError::Usage(_))));
    }

    #[test]
    fn validate_pin_requires_mode() {
        let no_mode = PinArgs {
            chat: "me".to_string(),
            id: None,
            unpin: false,
            notify: false,
            show: false,
            all: false,
        };
        assert!(matches!(validate_pin(&no_mode), Err(TeleError::Usage(_))));
        let with_id = PinArgs {
            chat: "me".to_string(),
            id: Some(7),
            unpin: false,
            notify: false,
            show: false,
            all: false,
        };
        assert!(validate_pin(&with_id).is_ok());
        let show = PinArgs {
            chat: "me".to_string(),
            id: None,
            unpin: false,
            notify: false,
            show: true,
            all: false,
        };
        assert!(validate_pin(&show).is_ok());
    }

    #[test]
    fn validate_chunk_size_kb_bounds_and_alignment() {
        assert!(validate_chunk_size_kb(4).is_ok());
        assert!(validate_chunk_size_kb(512).is_ok());
        assert!(validate_chunk_size_kb(128).is_ok());
        assert!(matches!(
            validate_chunk_size_kb(0),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_chunk_size_kb(5),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_chunk_size_kb(600),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn react_rejects_reaction_with_remove() {
        let both = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: Some("+1".to_string()),
            remove: true,
        };
        let err = validate_react(&both).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("not both"), "{}", err.message());
    }

    #[test]
    fn react_requires_reaction_or_remove() {
        let none = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: None,
            remove: false,
        };
        assert!(matches!(validate_react(&none), Err(TeleError::Usage(_))));
        let with_remove = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: None,
            remove: true,
        };
        assert!(validate_react(&with_remove).is_ok());
        let with_reaction = ReactArgs {
            chat: "me".to_string(),
            id: 5,
            reaction: Some("+1".to_string()),
            remove: false,
        };
        assert!(validate_react(&with_reaction).is_ok());
    }

    #[test]
    fn markdown_accepts_angle_bracket_link_dest() {
        let mut args = send_args("markdown");
        args.text = Some("[x](<tg://user?id=12345678>)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn markdown_rejects_junk_embedded_in_bare_link_dest() {
        let mut args = send_args("markdown");
        for bad in [
            "[x](tg://user?id=123\"456)",
            "[x](tg://user?id=123>456)",
            "[x](tg://user?id=123<456)",
            "[x](tg://user?id=123+456)",
        ] {
            args.text = Some(bad.to_string());
            assert!(
                matches!(validate_send(&args), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn markdown_accepts_id_terminated_by_space_like_grammers() {
        let mut args = send_args("markdown");
        args.text = Some("[x](tg://user?id=123 456)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn markdown_validates_every_link_in_text() {
        let mut args = send_args("markdown");
        args.text = Some("[a](tg://user?id=1)[b](tg://user?id=abc)".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.text = Some("[a](tg://user?id=1)[b](tg://user?id=2)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn markdown_rejects_overflowing_and_accepts_max_mention_ids() {
        let mut args = send_args("markdown");
        args.text = Some("[x](tg://user?id=99999999999999999999999)".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.text = Some("[x](tg://user?id=9223372036854775807)".to_string());
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn schedule_rejects_now_and_recent_past() {
        let now = chrono::Utc::now().timestamp();
        for v in [(now - 1).to_string(), now.to_string(), "+3600".to_string()] {
            assert!(
                matches!(parse_schedule(Some(&v)), Err(TeleError::Usage(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn schedule_accepts_max_i32_boundary() {
        assert_eq!(
            parse_schedule(Some("2147483647")).unwrap().unwrap(),
            i32::MAX
        );
    }

    #[test]
    fn schedule_rejects_empty_and_leap_second_strings() {
        for v in ["", "2016-12-31T23:59:60Z"] {
            assert!(
                matches!(parse_schedule(Some(v)), Err(TeleError::Usage(_))),
                "{v:?}"
            );
        }
    }

    #[test]
    fn schedule_normalizes_rfc3339_offsets_before_comparing() {
        let future = chrono::Utc::now() + chrono::Duration::hours(2);
        let with_offset = future
            .with_timezone(&chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap())
            .to_rfc3339();
        assert!(parse_schedule(Some(&with_offset)).unwrap().is_some());
        let past = chrono::Utc::now() - chrono::Duration::hours(2);
        let with_offset = past
            .with_timezone(&chrono::FixedOffset::west_opt(8 * 3600).unwrap())
            .to_rfc3339();
        assert!(matches!(
            parse_schedule(Some(&with_offset)),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn batches_splits_101_into_two_and_1000_into_ten() {
        let ids: Vec<i32> = (1..=101).collect();
        assert_eq!(
            batches(&ids).iter().map(|b| b.len()).collect::<Vec<_>>(),
            vec![100, 1]
        );
        let ids: Vec<i32> = (1..=1000).collect();
        let parts = batches(&ids);
        assert_eq!(parts.len(), 10);
        assert!(parts.iter().all(|b| b.len() == 100));
    }

    #[test]
    fn upload_rejects_any_sensitive_basename_and_allows_lookalikes() {
        for bad in [
            "a.session",
            "a.SESSION",
            "a.session-journal",
            ".ENV",
            ".env",
        ] {
            assert!(
                matches!(
                    validate_upload_path(&format!("C:/tmp/{bad}")),
                    Err(TeleError::Usage(_))
                ),
                "{bad}"
            );
        }
        let dir = upload_fixture("lookalike", &["session", "env", "x.env"]);
        for good in ["session", "env", "x.env"] {
            assert!(
                validate_upload_path(&dir.join(good).to_string_lossy()).is_ok(),
                "{good}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upload_rejects_windows_alias_names() {
        for bad in [
            "file.txt.",
            "file.txt ",
            "file.txt:stream",
            "CON",
            "con.txt",
            "LPT1",
            "COM9",
            "NUL",
            "AUX",
            "PRN",
            "COM1",
            "LPT9",
        ] {
            assert!(
                matches!(
                    validate_upload_path(&format!("C:/tmp/{bad}")),
                    Err(TeleError::Usage(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn upload_allows_windows_lookalike_names() {
        let dir = upload_fixture(
            "wlookalike",
            &["file.txt", "CONs", "COM10", "report.txt", "com0", "lpt10"],
        );
        for good in ["file.txt", "CONs", "COM10", "report.txt", "com0", "lpt10"] {
            assert!(
                validate_upload_path(&dir.join(good).to_string_lossy()).is_ok(),
                "{good}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_filename_rejects_windows_aliases() {
        for bad in [
            "file.txt.",
            "file.txt ",
            "file.txt:stream",
            "CON",
            "con.txt",
            "LPT1",
            "COM9",
            "NUL",
            "aux",
            "prn",
            "com1",
            "lpt9",
        ] {
            assert!(
                matches!(validate_filename(bad), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn validate_filename_accepts_safe_names() {
        for good in [
            "file.txt",
            "CONs",
            "COM10",
            "report.txt",
            "com0",
            "lpt10",
            "a.b.c",
        ] {
            assert!(validate_filename(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn reserved_device_names_match_exact_base_names() {
        for stem in [
            "con", "CON", "prn", "aux", "nul", "com1", "COM9", "lpt1", "LPT9",
        ] {
            assert!(is_reserved_device_name(stem), "{stem}");
        }
        for stem in [
            "cons", "console", "com0", "com10", "lpt0", "lpt10", "aux2", "",
        ] {
            assert!(!is_reserved_device_name(stem), "{stem}");
        }
    }

    #[test]
    fn sanitize_download_name_strips_parent_traversal() {
        assert_eq!(sanitize_download_name("../x"), "x");
        assert_eq!(sanitize_download_name("a/../b"), "b");
        assert_eq!(sanitize_download_name("a\\..\\b"), "b");
        assert_eq!(sanitize_download_name(".."), "document.bin");
        assert_eq!(sanitize_download_name("."), "document.bin");
    }

    #[test]
    fn sanitize_download_name_strips_trailing_dots_and_spaces() {
        assert_eq!(sanitize_download_name("name.."), "name");
        assert_eq!(sanitize_download_name("name "), "name");
        assert_eq!(sanitize_download_name("name. "), "name");
        assert_eq!(sanitize_download_name("..."), "document.bin");
    }

    #[test]
    fn sanitize_download_name_keeps_valid_names_unchanged() {
        assert_eq!(sanitize_download_name("report.pdf"), "report.pdf");
        assert_eq!(sanitize_download_name("عکس.png"), "عکس.png");
        assert_eq!(sanitize_download_name("photo.jpg"), "photo.jpg");
    }

    #[test]
    fn sanitize_download_name_falls_back_on_empty() {
        assert_eq!(sanitize_download_name(""), "document.bin");
    }

    #[test]
    fn sanitize_download_name_maps_reserved_device_names() {
        for (input, expected) in [
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("NUL", "_NUL"),
            ("nul.log", "_nul.log"),
            ("PRN", "_PRN"),
            ("AUX", "_AUX"),
            ("COM1", "_COM1"),
            ("com3.bin", "_com3.bin"),
            ("LPT9", "_LPT9"),
            ("lpt5.txt", "_lpt5.txt"),
            ("CON.", "_CON"),
            ("COM10", "COM10"),
            ("LPT10", "LPT10"),
            ("conference.pdf", "conference.pdf"),
            ("company.1", "company.1"),
        ] {
            assert_eq!(sanitize_download_name(input), expected, "{input}");
        }
    }

    #[test]
    fn download_temp_path_is_dot_prefixed_in_same_dir() {
        let dir = std::env::temp_dir().join(format!("telecli-dl-tmp-{}", std::process::id()));
        let final_path = dir.join("report.pdf");
        let temp = download_temp_path(&final_path);
        assert_eq!(temp.parent().unwrap(), dir);
        let fname = temp.file_name().unwrap().to_string_lossy().to_string();
        assert!(fname.starts_with(".report.pdf.part-"), "{fname}");
        assert!(fname
            .strip_prefix(".report.pdf.part-")
            .unwrap()
            .parse::<u32>()
            .is_ok());
    }

    #[test]
    fn refuse_existing_download_target_rejects_existing_file() {
        let base = std::env::temp_dir().join(format!("telecli-dl-exists-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let existing = base.join("x.bin");
        std::fs::write(&existing, b"old").unwrap();
        assert!(matches!(
            refuse_existing_download_target(&existing),
            Err(TeleError::Usage(_))
        ));
        let fresh = base.join("y.bin");
        assert!(refuse_existing_download_target(&fresh).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_download_temp_refuses_existing_temp() {
        let base = std::env::temp_dir().join(format!("telecli-dl-ct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let temp = base.join(".a.part-1");
        assert!(create_download_temp(&temp).is_ok());
        assert!(matches!(
            create_download_temp(&temp),
            Err(TeleError::Other(_))
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commit_download_moves_temp_into_place() {
        let base = std::env::temp_dir().join(format!("telecli-dl-commit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let temp = base.join(".x.bin.part-1");
        let final_path = base.join("x.bin");
        std::fs::write(&temp, b"payload").unwrap();
        commit_download(&temp, &final_path).unwrap();
        assert_eq!(std::fs::read(&final_path).unwrap(), b"payload");
        assert!(!temp.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commit_download_cleans_temp_on_failure() {
        let base = std::env::temp_dir().join(format!("telecli-dl-cfail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let temp = base.join(".x.bin.part-1");
        let occupied = base.join("occupied");
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(&temp, b"payload").unwrap();
        assert!(commit_download(&temp, &occupied).is_err());
        assert!(!temp.exists());
        assert!(occupied.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn download_dir_rejects_app_data_and_sessions() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-dl-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sessions")).unwrap();
        std::env::set_var("TELE_APP_DIR", &base);
        for bad in [
            base.to_string_lossy().to_string(),
            base.join("sessions").to_string_lossy().to_string(),
            base.join("sessions")
                .join("new")
                .to_string_lossy()
                .to_string(),
            base.join("export").to_string_lossy().to_string(),
            base.join("not-yet-created").to_string_lossy().to_string(),
        ] {
            assert!(
                matches!(validate_download_dir(&bad), Err(TeleError::Usage(_))),
                "{bad}"
            );
        }
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn download_dir_allows_dirs_outside_app_data() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-dl-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::env::set_var("TELE_APP_DIR", &base);
        let sibling = std::env::temp_dir().join(format!(
            "telecli-msg-dl-guard-{}-outside",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&sibling);
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(validate_download_dir(&sibling.to_string_lossy()).is_ok());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn download_dir_resolves_nonexisting_tail_outside_app_data() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-dl-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::env::set_var("TELE_APP_DIR", &base);
        let sibling = std::env::temp_dir().join(format!(
            "telecli-msg-dl-guard-{}-outside",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&sibling);
        std::fs::create_dir_all(&sibling).unwrap();
        let outside = sibling.join("deep").join("not-yet-created");
        assert!(validate_download_dir(&outside.to_string_lossy()).is_ok());
        let inside = base.join("not-yet-created").join("nested");
        assert!(
            matches!(
                validate_download_dir(&inside.to_string_lossy()),
                Err(TeleError::Usage(_))
            ),
            "{inside:?}"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[cfg(windows)]
    #[test]
    fn download_dir_rejects_case_variant_of_app_data() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-dl-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let lower = base.to_string_lossy().to_lowercase();
        std::env::set_var("TELE_APP_DIR", &lower);
        let candidate = base.join("sessions").join("new");
        assert!(
            matches!(
                validate_download_dir(&candidate.to_string_lossy()),
                Err(TeleError::Usage(_))
            ),
            "{candidate:?}"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn upload_rejects_case_variant_app_data_path() {
        let _guard = crate::config::TEST_ENV_LOCK.blocking_lock();
        let base =
            std::env::temp_dir().join(format!("telecli-msg-upload-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let lower = base.to_string_lossy().to_lowercase();
        std::env::set_var("TELE_APP_DIR", &lower);
        let candidate = base.join("secret.txt");
        assert!(
            matches!(
                validate_upload_path(&candidate.to_string_lossy()),
                Err(TeleError::Usage(_))
            ),
            "{candidate:?}"
        );
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn upload_rejects_nonexistent_file() {
        let dir = upload_fixture("missing", &[]);
        let missing = dir.join("definitely-missing.txt");
        assert!(matches!(
            validate_upload_path(&missing.to_string_lossy()),
            Err(TeleError::Usage(msg)) if msg.contains("not found")
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
        crate::config::TEST_ENV_LOCK.lock().await
    }

    fn dryrun_flags(command: &str, dry_run: bool) -> GlobalFlags {
        GlobalFlags {
            account: vec!["me".to_string()],
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run,
            quiet: true,
            config_path: None,
            command: command.to_string(),
        }
    }

    fn fake_app_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("telecli-msg-dryrun-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(dir.join("sessions").join("me.session"), b"").unwrap();
        dir
    }

    #[tokio::test]
    async fn get_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = get(
            GetArgs {
                chat: "me".to_string(),
                id: None,
                limit: 10,
                offset_id: None,
                last: false,
            },
            &dryrun_flags("msg get", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn get_without_dry_run_requires_a_real_session() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = get(
            GetArgs {
                chat: "me".to_string(),
                id: None,
                limit: 10,
                offset_id: None,
                last: false,
            },
            &dryrun_flags("msg get", false),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(code, 0);
    }

    #[tokio::test]
    async fn search_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = search(
            SearchArgs {
                chat: "me".to_string(),
                query: "hello".to_string(),
                limit: 10,
                global: false,
            },
            &dryrun_flags("msg search", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn search_global_dry_run_skips_chat_requirement() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = search(
            SearchArgs {
                chat: String::new(),
                query: "hello".to_string(),
                limit: 10,
                global: true,
            },
            &dryrun_flags("msg search", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn search_without_dry_run_requires_a_real_session() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = search(
            SearchArgs {
                chat: "me".to_string(),
                query: "hello".to_string(),
                limit: 10,
                global: false,
            },
            &dryrun_flags("msg search", false),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(code, 0);
    }

    #[tokio::test]
    async fn download_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let out =
            std::env::temp_dir().join(format!("telecli-msg-dl-dryrun-{}", std::process::id()));
        let code = download(
            DownloadArgs {
                chat: "me".to_string(),
                id: 1,
                dir: out.to_string_lossy().to_string(),
                force: false,
                chunk_size_kb: None,
            },
            &dryrun_flags("msg download", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[test]
    fn download_force_flag_allows_overwrite() {
        let dir = temp_path("dl-force");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("existing.txt");
        std::fs::write(&target, b"old").unwrap();
        assert!(refuse_existing_download_target(&target).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_dry_run_carries_argument_keys() {
        let mut args = send_args("plain");
        args.chat = "@x".to_string();
        let value = send_dry_run_payload(&args, None);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["chat"], serde_json::json!("@x"));
        assert_eq!(value["text"], serde_json::json!("hi"));
        assert_eq!(value["file"], serde_json::Value::Null);
        assert_eq!(value["caption"], serde_json::Value::Null);
        assert_eq!(value["format"], serde_json::json!("plain"));
        assert_eq!(value["schedule"], serde_json::Value::Null);
        assert_eq!(value["reply"], serde_json::Value::Null);
        assert_eq!(value["preview"], serde_json::json!(true));
        assert_eq!(value["silent"], serde_json::json!(false));
        assert_eq!(value["would"], serde_json::json!("send message to chat @x"));
    }

    #[test]
    fn validate_send_rejects_nonpositive_topic() {
        let mut args = send_args("plain");
        args.topic = Some(0);
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.topic = Some(-5);
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.topic = Some(7);
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn validate_send_album_bounds_and_conflicts() {
        let dir = upload_fixture("album-bounds", &["a.pdf"]);
        let mut args = send_args("plain");
        args.text = None;
        let p = dir.join("a.pdf").to_string_lossy().into_owned();
        args.files = vec![p.clone()];
        assert!(validate_send(&args).is_ok());
        args.files = (0..10)
            .map(|i| dir.join(format!("f{i}.jpg")).to_string_lossy().into_owned())
            .collect();
        for i in 0..10 {
            std::fs::write(dir.join(format!("f{i}.jpg")), b"x").unwrap();
        }
        assert!(validate_send(&args).is_ok());
        let missing = dir.join("nope.jpg").to_string_lossy().into_owned();
        args.files.push(missing);
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_send_url_requires_kind_and_conflicts_with_text() {
        let url_only = SendArgs {
            chat: "me".to_string(),
            text: Some("hi".to_string()),
            schedule: None,
            files: vec![],
            media_ttl: None,
            thumbnail: None,
            url: Some("https://example.com/cat.jpg".to_string()),
            kind: Some("photo".to_string()),
            copy_from: None,
            copy_id: None,
            caption: None,
            reply: None,
            topic: None,
            preview: true,
            no_preview: false,
            format: "plain".to_string(),
            silent: false,
            noforwards: false,
            background: false,
        };
        assert!(matches!(validate_send(&url_only), Err(TeleError::Usage(_))));
        let with_kind = SendArgs {
            kind: Some("document".to_string()),
            ..clone_without_text_for_tests(&url_only)
        };
        assert!(validate_send(&with_kind).is_ok());
    }

    fn clone_without_text_for_tests(args: &SendArgs) -> SendArgs {
        let mut c = args.clone();
        c.text = None;
        c
    }

    #[test]
    fn validate_send_rejects_bad_media_ttl_and_kind() {
        let mut args = clone_without_text_for_tests(&send_args("plain"));
        args.url = Some("https://example.com/x.jpg".to_string());
        args.kind = Some("sticker".to_string());
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.kind = Some("photo".to_string());
        args.media_ttl = Some(0);
        assert!(matches!(validate_send(&args), Err(TeleError::Usage(_))));
        args.media_ttl = Some(60);
        assert!(validate_send(&args).is_ok());
    }

    #[test]
    fn send_rejects_no_preview_with_file() {
        let dir = upload_fixture("nopreview", &["a.pdf"]);
        let mut args = send_args("plain");
        args.text = None;
        args.files = vec![dir.join("a.pdf").to_string_lossy().into_owned()];
        args.no_preview = true;
        let err = validate_send(&args).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("--no-preview"), "{}", err.message());
        args.no_preview = false;
        assert!(validate_send(&args).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_dry_run_payload_carries_effective_preview() {
        let args = send_args("plain");
        assert_eq!(
            send_dry_run_payload(&args, None)["preview"],
            serde_json::json!(true)
        );
        let mut disabled = send_args("plain");
        disabled.no_preview = true;
        assert_eq!(
            send_dry_run_payload(&disabled, None)["preview"],
            serde_json::json!(false)
        );
        let mut both = send_args("plain");
        both.preview = false;
        both.no_preview = true;
        assert_eq!(
            send_dry_run_payload(&both, None)["preview"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn edit_dry_run_carries_argument_keys() {
        let args = EditArgs {
            chat: "@x".to_string(),
            id: 5,
            text: "new text".to_string(),
        };
        let value = edit_dry_run_payload(&args.chat, &args.text, args.id);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["id"], serde_json::json!(5));
        assert_eq!(value["chat"], serde_json::json!("@x"));
        assert_eq!(value["text"], serde_json::json!("new text"));
        assert_eq!(value["would"], serde_json::json!("edit message 5"));
    }

    fn vote_args(option: &str) -> VoteArgs {
        VoteArgs {
            chat: "me".to_string(),
            id: 5,
            option: option.to_string(),
        }
    }

    fn typing_args(action: Option<&str>) -> TypingArgs {
        TypingArgs {
            chat: "me".to_string(),
            action: action.map(str::to_string),
        }
    }

    fn click_args(button: Option<String>, button_index: Option<usize>) -> ClickArgs {
        ClickArgs {
            chat: "me".to_string(),
            id: 5,
            button,
            button_index,
            password: false,
        }
    }

    #[test]
    fn parse_vote_options_accepts_single_and_multiple() {
        assert_eq!(parse_vote_options("2").unwrap(), vec![2]);
        assert_eq!(parse_vote_options("1,3").unwrap(), vec![1, 3]);
        assert_eq!(parse_vote_options(" 2 , 1 ").unwrap(), vec![2, 1]);
    }

    #[test]
    fn parse_vote_options_rejects_garbage() {
        for bad in ["", " ", ",", "0", "-1", "a", "1,,2", "1.5", "2,", ",1"] {
            assert!(
                matches!(parse_vote_options(bad), Err(TeleError::Usage(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn parse_vote_options_rejects_duplicates() {
        let err = parse_vote_options("1,1").unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("duplicate"), "{}", err.message());
    }

    #[test]
    fn validate_vote_matrix() {
        assert!(validate_vote(&vote_args("1")).is_ok());
        let mut bad = vote_args("1");
        bad.chat = "  ".to_string();
        assert!(matches!(validate_vote(&bad), Err(TeleError::Usage(_))));
        let mut zero_id = vote_args("1");
        zero_id.id = 0;
        assert!(matches!(validate_vote(&zero_id), Err(TeleError::Usage(_))));
        assert!(matches!(
            validate_vote(&vote_args("0")),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_vote(&vote_args("x")),
            Err(TeleError::Usage(_))
        ));
    }

    #[tokio::test]
    async fn vote_requires_explicit_account_when_not_dry_run() {
        let flags = GlobalFlags {
            account: Vec::new(),
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: false,
            quiet: true,
            config_path: None,
            command: "msg vote".to_string(),
        };
        let err = vote(vote_args("1"), &flags).await.unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("--account"), "{}", err.message());
    }

    #[tokio::test]
    async fn click_requires_explicit_account_when_not_dry_run() {
        let flags = GlobalFlags {
            account: Vec::new(),
            tag: Vec::new(),
            parallel: None,
            json: true,
            jsonl: false,
            dry_run: false,
            quiet: true,
            config_path: None,
            command: "msg click".to_string(),
        };
        let err = click(click_args(Some("Yes".to_string()), None), &flags)
            .await
            .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("--account"), "{}", err.message());
    }

    #[tokio::test]
    async fn vote_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = vote(vote_args("1,3"), &dryrun_flags("msg vote", true))
            .await
            .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    #[test]
    fn typing_action_allowlist_maps_known_names() {
        assert!(matches!(typing_action(None).unwrap(), TypingChoice::Typing));
        assert!(matches!(
            typing_action(Some("typing")).unwrap(),
            TypingChoice::Typing
        ));
        assert!(matches!(
            typing_action(Some("upload-photo")).unwrap(),
            TypingChoice::UploadPhoto
        ));
        assert!(matches!(
            typing_action(Some("upload-file")).unwrap(),
            TypingChoice::UploadFile
        ));
        assert!(matches!(
            typing_action(Some("cancel")).unwrap(),
            TypingChoice::Cancel
        ));
    }

    #[test]
    fn typing_action_rejects_unknown_with_valid_list() {
        for bad in ["giphy", "", "TYPING LOUDLY", "upload"] {
            let err = typing_action(Some(bad)).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "{bad:?}");
            assert!(err.message().contains("upload-photo"), "{bad:?}");
        }
    }

    #[test]
    fn validate_typing_matrix() {
        assert!(validate_typing(&typing_args(None)).is_ok());
        assert!(validate_typing(&typing_args(Some("cancel"))).is_ok());
        let mut bad = typing_args(None);
        bad.chat = String::new();
        assert!(matches!(validate_typing(&bad), Err(TeleError::Usage(_))));
    }

    #[tokio::test]
    async fn typing_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = typing(
            typing_args(Some("upload-photo")),
            &dryrun_flags("msg typing", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    fn inline_markup_json(
        rows: Vec<Vec<grammers_client::tl::enums::KeyboardButton>>,
    ) -> serde_json::Value {
        crate::serialize::reply_markup_to_json(
            &grammers_client::tl::enums::ReplyMarkup::ReplyInlineMarkup(
                grammers_client::tl::types::ReplyInlineMarkup {
                    rows: rows
                        .into_iter()
                        .map(|buttons| {
                            grammers_client::tl::enums::KeyboardButtonRow::Row(
                                grammers_client::tl::types::KeyboardButtonRow { buttons },
                            )
                        })
                        .collect(),
                },
            ),
        )
    }

    fn reply_keyboard_json(
        rows: Vec<Vec<grammers_client::tl::enums::KeyboardButton>>,
    ) -> serde_json::Value {
        crate::serialize::reply_markup_to_json(
            &grammers_client::tl::enums::ReplyMarkup::ReplyKeyboardMarkup(
                grammers_client::tl::types::ReplyKeyboardMarkup {
                    resize: false,
                    single_use: false,
                    selective: false,
                    persistent: false,
                    rows: rows
                        .into_iter()
                        .map(|buttons| {
                            grammers_client::tl::enums::KeyboardButtonRow::Row(
                                grammers_client::tl::types::KeyboardButtonRow { buttons },
                            )
                        })
                        .collect(),
                    placeholder: None,
                },
            ),
        )
    }

    fn hide_markup_json() -> serde_json::Value {
        crate::serialize::reply_markup_to_json(
            &grammers_client::tl::enums::ReplyMarkup::ReplyKeyboardHide(
                grammers_client::tl::types::ReplyKeyboardHide { selective: false },
            ),
        )
    }

    fn tl_text_button(text: &str) -> grammers_client::tl::enums::KeyboardButton {
        grammers_client::tl::enums::KeyboardButton::Button(
            grammers_client::tl::types::KeyboardButton {
                style: None,
                text: text.into(),
            },
        )
    }

    fn tl_url_button(text: &str, url: &str) -> grammers_client::tl::enums::KeyboardButton {
        grammers_client::tl::enums::KeyboardButton::Url(
            grammers_client::tl::types::KeyboardButtonUrl {
                style: None,
                text: text.into(),
                url: url.into(),
            },
        )
    }

    fn tl_callback_button(text: &str, data: &[u8]) -> grammers_client::tl::enums::KeyboardButton {
        grammers_client::tl::enums::KeyboardButton::Callback(
            grammers_client::tl::types::KeyboardButtonCallback {
                requires_password: false,
                style: None,
                text: text.into(),
                data: data.to_vec(),
            },
        )
    }

    fn two_row_inline_markup() -> serde_json::Value {
        inline_markup_json(vec![
            vec![
                tl_callback_button("Yes", b"q:yes"),
                tl_url_button("Docs", "https://example.com"),
            ],
            vec![
                tl_callback_button("No", b"q:no"),
                tl_callback_button("Maybe", b"q:maybe"),
            ],
        ])
    }

    #[test]
    fn locate_button_finds_callback_by_exact_text() {
        let found =
            locate_button(&two_row_inline_markup(), &ButtonSelector::Text("No".into())).unwrap();
        assert_eq!(found.position, 3);
        assert_eq!(found.text, "No");
        assert_eq!(found.callback_data, b"q:no".to_vec());
    }

    #[test]
    fn locate_button_finds_by_index_across_rows() {
        let found = locate_button(&two_row_inline_markup(), &ButtonSelector::Index(1)).unwrap();
        assert_eq!(found.position, 1);
        assert_eq!(found.text, "Yes");
        assert_eq!(found.callback_data, b"q:yes".to_vec());
        let last = locate_button(&two_row_inline_markup(), &ButtonSelector::Index(4)).unwrap();
        assert_eq!(last.text, "Maybe");
        let url_hit =
            locate_button(&two_row_inline_markup(), &ButtonSelector::Index(2)).unwrap_err();
        assert!(
            url_hit.message().contains("callback"),
            "{}",
            url_hit.message()
        );
    }

    #[test]
    fn locate_button_matches_case_insensitive_when_unique() {
        let found = locate_button(
            &two_row_inline_markup(),
            &ButtonSelector::Text("maybe".into()),
        )
        .unwrap();
        assert_eq!(found.callback_data, b"q:maybe".to_vec());
    }

    #[test]
    fn locate_button_reports_ambiguous_text_match() {
        let markup = inline_markup_json(vec![vec![
            tl_callback_button("ok", b"a"),
            tl_callback_button("ok", b"b"),
        ]]);
        let err = locate_button(&markup, &ButtonSelector::Text("ok".into())).unwrap_err();
        assert!(
            err.message().contains("--button-index"),
            "{}",
            err.message()
        );
        assert!(err.message().contains("[1, 2]"), "{}", err.message());
    }

    #[test]
    fn locate_button_prefers_exact_over_case_insensitive_match() {
        let markup = inline_markup_json(vec![vec![
            tl_callback_button("ok", b"a"),
            tl_callback_button("OK", b"b"),
        ]]);
        let found = locate_button(&markup, &ButtonSelector::Text("ok".into())).unwrap();
        assert_eq!(found.callback_data, b"a".to_vec());
    }

    #[test]
    fn locate_button_errors_on_missing_text() {
        let err = locate_button(
            &two_row_inline_markup(),
            &ButtonSelector::Text("Zilch".into()),
        )
        .unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(err.message().contains("Zilch"), "{}", err.message());
    }

    #[test]
    fn locate_button_rejects_reply_keyboard_with_send_hint() {
        let markup = reply_keyboard_json(vec![
            vec![tl_text_button("Ping")],
            vec![tl_text_button("Pong")],
        ]);
        let err = locate_button(&markup, &ButtonSelector::Text("Ping".into())).unwrap_err();
        assert!(err.message().contains("not clickable"), "{}", err.message());
        assert!(err.message().contains("msg send"), "{}", err.message());
    }

    #[test]
    fn locate_button_handles_empty_markup_kinds() {
        let err = locate_button(&hide_markup_json(), &ButtonSelector::Index(1)).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        let force_reply = crate::serialize::reply_markup_to_json(
            &grammers_client::tl::enums::ReplyMarkup::ReplyKeyboardForceReply(
                grammers_client::tl::types::ReplyKeyboardForceReply {
                    single_use: false,
                    selective: false,
                    placeholder: None,
                },
            ),
        );
        assert!(locate_button(&force_reply, &ButtonSelector::Index(1)).is_err());
    }

    #[test]
    fn locate_button_rejects_zero_and_out_of_range_index() {
        let zero = locate_button(&two_row_inline_markup(), &ButtonSelector::Index(0)).unwrap_err();
        assert!(zero.message().contains("1-based"), "{}", zero.message());
        let over = locate_button(&two_row_inline_markup(), &ButtonSelector::Index(9)).unwrap_err();
        assert!(over.message().contains('9'), "{}", over.message());
    }

    #[test]
    fn validate_click_requires_exactly_one_selector() {
        assert!(validate_click(&click_args(Some("Yes".to_string()), None)).is_ok());
        assert!(validate_click(&click_args(None, Some(2))).is_ok());
        let none = click_args(None, None);
        assert!(matches!(validate_click(&none), Err(TeleError::Usage(_))));
        let both = click_args(Some("Yes".to_string()), Some(2));
        assert!(matches!(validate_click(&both), Err(TeleError::Usage(_))));
        let mut empty_chat = click_args(None, Some(1));
        empty_chat.chat = "".to_string();
        assert!(matches!(
            validate_click(&empty_chat),
            Err(TeleError::Usage(_))
        ));
        let mut zero = click_args(None, Some(0));
        zero.id = 0;
        assert!(matches!(validate_click(&zero), Err(TeleError::Usage(_))));
        let mut idx0 = click_args(None, Some(0));
        idx0.button_index = Some(0);
        assert!(matches!(validate_click(&idx0), Err(TeleError::Usage(_))));
    }

    #[test]
    fn validate_click_password_fails_honestly() {
        let mut args = click_args(Some("Yes".to_string()), None);
        args.password = true;
        let err = validate_click(&args).unwrap_err();
        assert!(err.message().contains("password"), "{}", err.message());
        assert!(err.message().contains("not supported"), "{}", err.message());
    }

    #[tokio::test]
    async fn click_dry_run_short_circuits_before_connect() {
        let _guard = lock_env().await;
        let dir = fake_app_dir();
        std::env::set_var("TELE_APP_DIR", &dir);
        let code = click(
            click_args(Some("Yes".to_string()), None),
            &dryrun_flags("msg click", true),
        )
        .await
        .unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0);
    }

    fn poll_media_with_answers() -> grammers_client::media::Poll {
        use grammers_client::tl;
        let answers = ["Alpha", "Beta", "Gamma"]
            .iter()
            .enumerate()
            .map(|(i, label)| {
                tl::enums::PollAnswer::Answer(tl::types::PollAnswer {
                    media: None,
                    added_by: None,
                    date: None,
                    text: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                        text: label.to_string(),
                        entities: Vec::new(),
                    }),
                    option: format!("opt{i}").into_bytes(),
                })
            })
            .collect();
        let media = tl::enums::MessageMedia::Poll(Box::new(tl::types::MessageMediaPoll {
            poll: tl::enums::Poll::Poll(tl::types::Poll {
                id: 7,
                closed: false,
                public_voters: true,
                multiple_choice: false,
                quiz: false,
                open_answers: false,
                revoting_disabled: false,
                shuffle_answers: false,
                hide_results_until_close: false,
                creator: false,
                subscribers_only: false,
                question: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                    text: "Pick one?".into(),
                    entities: Vec::new(),
                }),
                answers,
                close_period: None,
                close_date: None,
                countries_iso2: None,
                hash: 0,
            }),
            results: tl::enums::PollResults::Results(Box::new(tl::types::PollResults {
                min: false,
                has_unread_votes: false,
                can_view_stats: true,
                results: Some(vec![tl::enums::PollAnswerVoters::Voters(
                    tl::types::PollAnswerVoters {
                        chosen: true,
                        correct: false,
                        option: b"opt0".to_vec(),
                        voters: Some(12),
                        recent_voters: None,
                    },
                )]),
                total_voters: Some(30),
                recent_voters: None,
                solution: None,
                solution_entities: None,
                solution_media: None,
            })),
            attached_media: None,
        }));
        match grammers_client::media::Media::from_raw(media).unwrap() {
            grammers_client::media::Media::Poll(p) => p,
            _ => panic!("expected poll media"),
        }
    }

    #[test]
    fn resolve_vote_options_maps_indexes_to_bytes() {
        let answers = crate::serialize::poll_answers(&poll_media_with_answers());
        let picked = resolve_vote_options(&answers, &[1, 3]).unwrap();
        assert_eq!(picked, vec![b"opt0".to_vec(), b"opt2".to_vec()]);
        let out_of_range = resolve_vote_options(&answers, &[4]).unwrap_err();
        assert!(
            out_of_range.message().contains("4"),
            "{}",
            out_of_range.message()
        );
        assert!(matches!(
            resolve_vote_options(&answers, &[0]),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn message_random_id_is_nonzero_and_advances() {
        let a = message_random_id();
        let b = message_random_id();
        assert_ne!(a, 0);
        assert_ne!(b, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn validate_send_noforwards_supports_text_only() {
        let mut text_only = send_args("plain");
        text_only.noforwards = true;
        text_only.background = true;
        assert!(validate_send(&text_only).is_ok());

        let dir = upload_fixture("nofwd", &["a.pdf"]);
        let mut with_file = send_args("plain");
        with_file.text = None;
        with_file.noforwards = true;
        with_file.files = vec![dir.join("a.pdf").to_string_lossy().into_owned()];
        let err = validate_send(&with_file).unwrap_err();
        assert!(err.message().contains("--noforwards"), "{}", err.message());

        let mut with_url = send_args("plain");
        with_url.text = None;
        with_url.noforwards = true;
        with_url.url = Some("https://example.com/x.jpg".to_string());
        with_url.kind = Some("photo".to_string());
        assert!(matches!(validate_send(&with_url), Err(TeleError::Usage(_))));

        let mut with_copy = send_args("plain");
        with_copy.text = None;
        with_copy.noforwards = true;
        with_copy.copy_from = Some("@src".to_string());
        with_copy.copy_id = Some(1);
        assert!(matches!(
            validate_send(&with_copy),
            Err(TeleError::Usage(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_send_background_rejects_albums_allows_single_and_text() {
        let dir = upload_fixture("bgmod", &["a.jpg", "b.jpg"]);
        let paths: Vec<String> = ["a.jpg", "b.jpg"]
            .iter()
            .map(|n| dir.join(n).to_string_lossy().into_owned())
            .collect();
        let mut album = send_args("plain");
        album.text = None;
        album.background = true;
        album.files = paths;
        let err = validate_send(&album).unwrap_err();
        assert!(err.message().contains("--background"), "{}", err.message());

        let mut single = send_args("plain");
        single.text = None;
        single.background = true;
        single.files = vec![dir.join("a.jpg").to_string_lossy().into_owned()];
        assert!(validate_send(&single).is_ok());

        let mut text = send_args("plain");
        text.background = true;
        assert!(validate_send(&text).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_dry_run_payload_carries_send_mods() {
        let mut args = send_args("plain");
        args.noforwards = true;
        args.background = true;
        let value = send_dry_run_payload(&args, None);
        assert_eq!(value["noforwards"], serde_json::json!(true));
        assert_eq!(value["background"], serde_json::json!(true));
        let plain = send_dry_run_payload(&send_args("plain"), None);
        assert_eq!(plain["noforwards"], serde_json::json!(false));
        assert_eq!(plain["background"], serde_json::json!(false));
    }
}
