use clap::Args;

#[derive(Args, Clone)]
pub struct SendArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "message text (mutually exclusive with --file)")]
    pub(crate) text: Option<String>,
    #[arg(
        long,
        help = "send time: Unix timestamp or RFC3339 datetime (must be in the future)"
    )]
    pub(crate) schedule: Option<String>,
    #[arg(
        long = "file",
        value_name = "PATH",
        help = "file path(s) to upload; 2-10 paths send as an album (mutually exclusive with --text)"
    )]
    pub(crate) files: Vec<String>,
    #[arg(long, help = "caption for uploaded file(s) (requires --file)")]
    pub(crate) caption: Option<String>,
    #[arg(long, help = "message ID to reply to")]
    pub(crate) reply: Option<i32>,
    #[arg(
        long,
        help = "forum topic ID to post into (conflicts with --reply)",
        conflicts_with = "reply"
    )]
    pub(crate) topic: Option<i32>,
    #[arg(
        long,
        help = "auto-destruct media after N seconds",
        value_name = "SECS"
    )]
    pub(crate) media_ttl: Option<i32>,
    #[arg(
        long,
        help = "custom thumbnail path for document uploads",
        value_name = "PATH"
    )]
    pub(crate) thumbnail: Option<String>,
    #[arg(
        long,
        help = "upload a remote URL as media (with --kind photo|document)",
        value_name = "URL"
    )]
    pub(crate) url: Option<String>,
    #[arg(
        long,
        help = "media kind for --url: photo or document",
        requires = "url"
    )]
    pub(crate) kind: Option<String>,
    #[arg(
        long,
        help = "re-send media from this chat's message without forward header (requires --copy-id)",
        value_name = "CHAT"
    )]
    pub(crate) copy_from: Option<String>,
    #[arg(
        long,
        help = "message ID whose media is re-sent (requires --copy-from)",
        requires = "copy_from",
        value_name = "ID"
    )]
    pub(crate) copy_id: Option<i32>,
    #[arg(long, default_value_t = true, help = "show link preview")]
    pub(crate) preview: bool,
    #[arg(long, action = clap::ArgAction::SetTrue, help = "disable link preview")]
    pub(crate) no_preview: bool,
    #[arg(long, default_value = "plain", help = "text format: plain or markdown")]
    pub(crate) format: String,
    #[arg(long, help = "send without notification sound")]
    pub(crate) silent: bool,
    #[arg(
        long,
        help = "disallow forwarding and saving of this message (text sends only)"
    )]
    pub(crate) noforwards: bool,
    #[arg(
        long,
        help = "send as a background message (no notification sound even in unmuted chats)"
    )]
    pub(crate) background: bool,
}

#[derive(Args, Clone)]
pub struct EditArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "message ID to edit")]
    pub(crate) id: i32,
    #[arg(long, help = "new message text")]
    pub(crate) text: String,
}

#[derive(Args, Clone)]
pub struct DeleteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "comma-separated message IDs to delete"
    )]
    pub(crate) ids: Vec<i32>,
    #[arg(long, help = "delete all messages in chat")]
    pub(crate) all: bool,
    #[arg(
        long,
        help = "delete only for yourself (no revoke; private chats and basic groups only, not channels)"
    )]
    pub(crate) self_only: bool,
}

#[derive(Args, Clone)]
pub struct ForwardArgs {
    #[arg(long, help = "source chat to forward from")]
    pub(crate) from: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "comma-separated message IDs to forward"
    )]
    pub(crate) ids: Vec<i32>,
    #[arg(long, help = "destination chat to forward to")]
    pub(crate) to: String,
}

#[derive(Args, Clone)]
pub struct PinArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "message ID to pin or unpin")]
    pub(crate) id: Option<i32>,
    #[arg(long, help = "remove pin instead of adding")]
    pub(crate) unpin: bool,
    #[arg(
        long,
        help = "notify chat members when pinning",
        conflicts_with_all = &["unpin", "show", "all"]
    )]
    pub(crate) notify: bool,
    #[arg(
        long,
        help = "show the currently pinned message",
        conflicts_with_all = &["id", "unpin", "all", "notify"]
    )]
    pub(crate) show: bool,
    #[arg(
        long,
        help = "unpin all pinned messages",
        conflicts_with_all = &["id", "unpin", "show", "notify"]
    )]
    pub(crate) all: bool,
}

#[derive(Args, Clone)]
pub struct GetArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(
        long,
        help = "fetch a single message by ID (conflicts with a msg id carried in --chat)"
    )]
    pub(crate) id: Option<i32>,
    #[arg(long, default_value_t = 10, help = "max results to return (1-10000)")]
    pub(crate) limit: u32,
    #[arg(long, help = "fetch messages before this ID")]
    pub(crate) offset_id: Option<i32>,
    #[arg(long, help = "fetch only the most recent message")]
    pub(crate) last: bool,
}

#[derive(Args, Clone)]
pub struct ReadArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "mark as unread instead of read")]
    pub(crate) mark_unread: bool,
    #[arg(
        long,
        help = "clear the mention badge only",
        conflicts_with = "mark_unread"
    )]
    pub(crate) mentions: bool,
}

#[derive(Args, Clone)]
pub struct ReactArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "message ID to react to")]
    pub(crate) id: i32,
    #[arg(long, help = "emoji reaction to add")]
    pub(crate) reaction: Option<String>,
    #[arg(long, help = "remove reaction instead of adding")]
    pub(crate) remove: bool,
}

#[derive(Args, Clone)]
pub struct SearchArgs {
    #[arg(
        long,
        default_value = "",
        help = "target chat: @username, t.me link, numeric ID, +phone, or me (not required with --global)"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "search query text")]
    pub(crate) query: String,
    #[arg(long, default_value_t = 10, help = "max results to return (1-10000)")]
    pub(crate) limit: u32,
    #[arg(
        long,
        help = "search across all dialogs instead of one chat",
        requires = "query"
    )]
    pub(crate) global: bool,
}

#[derive(Args, Clone)]
pub struct DownloadArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "message ID to download media from")]
    pub(crate) id: i32,
    #[arg(long, help = "output directory for downloaded media")]
    pub(crate) dir: String,
    #[arg(long, help = "overwrite existing files")]
    pub(crate) force: bool,
    #[arg(long, help = "streaming chunk size in KB (4-512, multiple of 4)")]
    pub(crate) chunk_size_kb: Option<usize>,
}

#[derive(Args, Clone)]
pub struct VoteArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "message ID of the poll")]
    pub(crate) id: i32,
    #[arg(
        long,
        help = "1-based option indexes to vote for, e.g. 1 or 1,3 (multiple only for multi-choice polls)"
    )]
    pub(crate) option: String,
}

#[derive(Args, Clone)]
pub struct TypingArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(
        long,
        help = "chat action: typing | upload-photo | upload-file | cancel (default typing; actions auto-expire)"
    )]
    pub(crate) action: Option<String>,
}

#[derive(Args, Clone)]
pub struct ClickArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: String,
    #[arg(long, help = "message ID carrying the inline keyboard")]
    pub(crate) id: i32,
    #[arg(
        long,
        value_name = "TEXT",
        help = "inline button text to click (exact match; precedence: --button-index > --button-contains > --button)",
        conflicts_with_all = ["button_index", "button_contains"]
    )]
    pub(crate) button: Option<String>,
    #[arg(
        long,
        value_name = "N",
        help = "1-based inline button position across all rows (precedence: --button-index > --button-contains > --button)",
        conflicts_with_all = ["button", "button_contains"]
    )]
    pub(crate) button_index: Option<usize>,
    #[arg(
        long,
        value_name = "SUBSTRING",
        help = "case-insensitive substring to match against button text (picks first match; precedence: --button-index > --button-contains > --button)",
        conflicts_with_all = ["button", "button_index"]
    )]
    pub(crate) button_contains: Option<String>,
    #[arg(
        long,
        help = "reserved for 2FA-protected buttons; not supported at this layer"
    )]
    pub(crate) password: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
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

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct ClickParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    pub(crate) button: Option<String>,
    pub(crate) button_index: Option<usize>,
    pub(crate) button_contains: Option<String>,
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
            button_contains: a.button_contains.clone(),
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
            button_contains: p.button_contains.clone(),
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
