use clap::{Args, Subcommand};

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::commands::validate_limit;
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum CacheCmd {
    #[command(about = "sync recent messages from a chat into the local cache")]
    Sync(SyncArgs),
    #[command(about = "search the local message cache offline (FTS5)")]
    Search(SearchArgs),
    #[command(about = "show local cache statistics")]
    Stats(StatsArgs),
    #[command(about = "clear the local message cache")]
    Clear(ClearArgs),
}

#[derive(Args, Clone)]
pub struct SyncArgs {
    #[arg(
        long,
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    chat: String,
    #[arg(long, default_value_t = 100, help = "max messages to sync (1-10000)")]
    limit: u32,
}

#[derive(Args, Clone)]
pub struct SearchArgs {
    #[arg(
        long,
        default_value = "",
        help = "full-text query (empty lists recent)"
    )]
    query: String,
    #[arg(long, help = "restrict to a numeric chat id")]
    chat_id: Option<i64>,
    #[arg(long, default_value_t = 20, help = "max results (1-10000)")]
    limit: u32,
}

#[derive(Args, Clone)]
pub struct StatsArgs {}

#[derive(Args, Clone)]
pub struct ClearArgs {}

pub async fn run(cmd: CacheCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        CacheCmd::Sync(a) => sync(a, flags).await,
        CacheCmd::Search(a) => search(a, flags).await,
        CacheCmd::Stats(a) => stats(a, flags).await,
        CacheCmd::Clear(a) => clear(a, flags).await,
    }
}

fn sync_dry_run_data(chat: &str, limit: u32) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "chat": chat,
        "limit": limit,
        "would": format!("sync up to {limit} messages from chat {chat} into local cache")})
}

async fn sync(args: SyncArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_limit(args.limit, 10_000, "limit")?;
    crate::chat_target::ChatTarget::parse_flag(&args.chat, "chat")?;
    crate::executor::require_explicit_selection("cache sync", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(sync_dry_run_data(&args.chat, args.limit));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            cache_sync_core(&guard.shares(), SyncParams::from(&args), &name).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn cache_sync_core(
    shares: &crate::client::ServeShares,
    params: SyncParams,
    account: &str,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let chat_ref = entities::peer_ref(&chat).await.map_err(tele_invocation)?;
    let chat_id = chat.id().bare_id().unwrap_or_default();
    let chat_name = crate::serialize::peer_name(&chat);
    let mut iter = shares.client.iter_messages(chat_ref);
    iter = iter.limit(params.limit as usize);
    let mut cached = Vec::new();
    while let Some(msg) = iter.next().await.map_err(tele_invocation)? {
        cached.push(crate::cache_db::CachedMessage {
            id: msg.id(),
            chat_id,
            chat_name: chat_name.clone(),
            sender_id: msg.sender().and_then(|s| s.id().bare_id()),
            sender_name: msg
                .sender()
                .map(crate::serialize::peer_name)
                .unwrap_or_default(),
            date: msg.date().timestamp(),
            text: msg.text().to_string(),
            media_kind: msg
                .media()
                .map(|m| crate::serialize::media_kind(&m).to_string()),
        });
    }
    let stored = crate::cache_db::store_messages(account, &cached).await?;
    Ok(serde_json::json!({ "chat": params.chat, "synced": stored }))
}

fn search_dry_run_data(query: &str, limit: u32) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "query": query,
        "limit": limit,
        "would": format!("search local cache for {query:?} (limit {limit})")})
}

async fn search(args: SearchArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_limit(args.limit, 10_000, "limit")?;
    crate::executor::require_explicit_selection("cache search", flags)?;
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(search_dry_run_data(&args.query, args.limit));
            }
            let result = cache_search_core(&SearchParams::from(&args), &name).await?;
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                let rows: Vec<Vec<String>> = result["messages"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(|r| {
                        vec![
                            r["chat_name"].as_str().unwrap_or_default().to_string(),
                            r["sender_name"].as_str().unwrap_or_default().to_string(),
                            r["text"]
                                .as_str()
                                .unwrap_or_default()
                                .chars()
                                .take(60)
                                .collect(),
                        ]
                    })
                    .collect();
                output::print_account_table(&name, multi, &["chat", "from", "text"], &rows)?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) async fn cache_search_core(
    params: &SearchParams,
    account: &str,
) -> TeleResult<serde_json::Value> {
    let found =
        crate::cache_db::search_cache(account, &params.query, params.chat_id, params.limit).await?;
    let messages: Vec<serde_json::Value> = found
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "chat_id": m.chat_id,
                "chat_name": m.chat_name,
                "sender_id": m.sender_id,
                "sender_name": m.sender_name,
                "date": m.date,
                "text": m.text,
                "media_kind": m.media_kind,
            })
        })
        .collect();
    Ok(serde_json::json!({ "query": params.query, "messages": messages }))
}

async fn stats(_args: StatsArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::executor::require_explicit_selection("cache stats", flags)?;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        Box::pin(async move {
            let result = crate::cache_db::cache_stats(&name).await?;
            if !output::machine_mode(json, jsonl) {
                output::print_account_table(
                    &name,
                    multi,
                    &["messages", "chats", "bytes"],
                    &[vec![
                        result["messages"].to_string(),
                        result["chats"].to_string(),
                        result["bytes"].to_string(),
                    ]],
                )?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn clear(_args: ClearArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::executor::require_explicit_selection("cache clear", flags)?;
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        Box::pin(async move {
            if dry_run {
                return Ok(serde_json::json!({
                    "dry_run": true,
                    "account": name,
                    "would": format!("clear local message cache for {name}")}));
            }
            crate::cache_db::clear_cache(&name).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct SyncParams {
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&SyncArgs> for SyncParams {
    fn from(a: &SyncArgs) -> Self {
        Self {
            chat: a.chat.clone(),
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&SyncParams> for SyncArgs {
    fn from(p: &SyncParams) -> Self {
        Self {
            chat: p.chat.clone(),
            limit: p.limit,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct SearchParams {
    #[serde(default)]
    pub(crate) query: String,
    pub(crate) chat_id: Option<i64>,
    #[serde(default)]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&SearchArgs> for SearchParams {
    fn from(a: &SearchArgs) -> Self {
        Self {
            query: a.query.clone(),
            chat_id: a.chat_id,
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&SearchParams> for SearchArgs {
    fn from(p: &SearchParams) -> Self {
        Self {
            query: p.query.clone(),
            chat_id: p.chat_id,
            limit: p.limit,
        }
    }
}

pub(crate) fn cache_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "cache sync",
            Lane::Mutate,
            Some(OP_TIMEOUT_PAGINATED),
            false,
            false,
            true,
            "sync recent messages from a chat into the local cache",
            SyncParams,
            SyncArgs,
            validate_sync,
            |a: &SyncArgs| Ok::<_, TeleError>(sync_dry_run_data(&a.chat, a.limit)),
            run_sync,
            crate::commands::serve::params_schema::<SyncParams>
        ),
        crate::serve_route!(
            "cache search",
            Lane::Read,
            Some(OP_TIMEOUT_SIMPLE),
            true,
            false,
            true,
            "search the local message cache offline",
            SearchParams,
            SearchArgs,
            validate_search,
            |a: &SearchArgs| Ok::<_, TeleError>(search_dry_run_data(&a.query, a.limit)),
            run_search_route,
            crate::commands::serve::params_schema::<SearchParams>
        ),
        crate::serve_route!(
            "cache stats",
            Lane::Read,
            Some(OP_TIMEOUT_SIMPLE),
            true,
            false,
            true,
            "show local cache statistics",
            StatsParams,
            StatsArgs,
            validate_stats,
            |_a: &StatsArgs| Ok::<_, TeleError>(
                serde_json::json!({"dry_run": true, "would": "show cache stats"})
            ),
            run_stats_route,
            crate::commands::serve::params_schema::<StatsParams>
        ),
        crate::serve_route!(
            "cache clear",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "clear the local message cache",
            ClearParams,
            ClearArgs,
            validate_clear,
            |_a: &ClearArgs| Ok::<_, TeleError>(
                serde_json::json!({"dry_run": true, "would": "clear cache"})
            ),
            run_clear_route,
            crate::commands::serve::params_schema::<ClearParams>
        ),
    ]
}

fn validate_sync(a: &SyncArgs) -> TeleResult<()> {
    validate_limit(a.limit, 10_000, "limit")?;
    crate::chat_target::ChatTarget::parse_flag(&a.chat, "chat")?;
    Ok(())
}

fn validate_search(a: &SearchArgs) -> TeleResult<()> {
    validate_limit(a.limit, 10_000, "limit")?;
    Ok(())
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct StatsParams {
    #[serde(default)]
    pub(crate) dry_run: bool,
}

fn validate_stats(_a: &StatsArgs) -> TeleResult<()> {
    Ok(())
}

impl From<&StatsParams> for StatsArgs {
    fn from(_p: &StatsParams) -> Self {
        Self {}
    }
}

impl From<&ClearParams> for ClearArgs {
    fn from(_p: &ClearParams) -> Self {
        Self {}
    }
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct ClearParams {
    #[serde(default)]
    pub(crate) dry_run: bool,
}

fn validate_clear(_a: &ClearArgs) -> TeleResult<()> {
    Ok(())
}

fn run_sync(
    shares: crate::client::ServeShares,
    raw: serde_json::Value,
) -> crate::commands::serve::ServeFuture {
    Box::pin(async move {
        let params: SyncParams = crate::commands::serve::deser_params(&raw).map_err(|f| {
            let mut err =
                crate::commands::serve::err_json("ServeError", format!("params: {}", f.message));
            if let Some(p) = f.param {
                err["param"] = serde_json::Value::from(p);
            }
            err
        })?;
        let account = shares.account_name().unwrap_or_default();
        cache_sync_core(&shares, params, &account)
            .await
            .map_err(|e| e.as_json())
    })
}

fn run_search_route(
    shares: crate::client::ServeShares,
    raw: serde_json::Value,
) -> crate::commands::serve::ServeFuture {
    Box::pin(async move {
        let params: SearchParams = crate::commands::serve::deser_params(&raw).map_err(|f| {
            let mut err =
                crate::commands::serve::err_json("ServeError", format!("params: {}", f.message));
            if let Some(p) = f.param {
                err["param"] = serde_json::Value::from(p);
            }
            err
        })?;
        let account = shares.account_name().unwrap_or_default();
        cache_search_core(&params, &account)
            .await
            .map_err(|e| e.as_json())
    })
}

fn run_stats_route(
    shares: crate::client::ServeShares,
    raw: serde_json::Value,
) -> crate::commands::serve::ServeFuture {
    Box::pin(async move {
        let _params: StatsParams = crate::commands::serve::deser_params(&raw).map_err(|f| {
            let mut err =
                crate::commands::serve::err_json("ServeError", format!("params: {}", f.message));
            if let Some(p) = f.param {
                err["param"] = serde_json::Value::from(p);
            }
            err
        })?;
        let account = shares.account_name().unwrap_or_default();
        crate::cache_db::cache_stats(&account)
            .await
            .map_err(|e| e.as_json())
    })
}

fn run_clear_route(
    shares: crate::client::ServeShares,
    raw: serde_json::Value,
) -> crate::commands::serve::ServeFuture {
    Box::pin(async move {
        let params: ClearParams = crate::commands::serve::deser_params(&raw).map_err(|f| {
            let mut err =
                crate::commands::serve::err_json("ServeError", format!("params: {}", f.message));
            if let Some(p) = f.param {
                err["param"] = serde_json::Value::from(p);
            }
            err
        })?;
        if params.dry_run {
            return Ok(serde_json::json!({"dry_run": true, "would": "clear cache"}));
        }
        let account = shares.account_name().unwrap_or_default();
        crate::cache_db::clear_cache(&account)
            .await
            .map_err(|e| e.as_json())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_dry_run_shape() {
        let v = sync_dry_run_data("@team", 50);
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["chat"], "@team");
        assert_eq!(v["limit"], 50);
        assert!(v["would"].as_str().unwrap().contains("@team"));
    }

    #[test]
    fn search_dry_run_shape() {
        let v = search_dry_run_data("deploy", 20);
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["query"], "deploy");
    }

    #[test]
    fn validate_sync_rejects_bad_limit() {
        let args = SyncArgs {
            chat: "@x".to_string(),
            limit: 0,
        };
        assert!(validate_sync(&args).is_err());
        let args = SyncArgs {
            chat: "@x".to_string(),
            limit: 10_001,
        };
        assert!(validate_sync(&args).is_err());
    }

    #[test]
    fn validate_sync_rejects_empty_chat() {
        let args = SyncArgs {
            chat: String::new(),
            limit: 10,
        };
        assert!(validate_sync(&args).is_err());
    }

    #[test]
    fn serve_routes_register_four_ops() {
        let routes = cache_serve_routes();
        assert_eq!(routes.len(), 4);
        let ops: Vec<&str> = routes.iter().map(|r| r.op).collect();
        assert!(ops.contains(&"cache sync"));
        assert!(ops.contains(&"cache search"));
        assert!(ops.contains(&"cache stats"));
        assert!(ops.contains(&"cache clear"));
    }
}
