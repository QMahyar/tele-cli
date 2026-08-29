use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, ServiceExt};

use crate::client::{ClientGuard, ServeShares};
use crate::commands::serve::{
    apply_confirm_gate, err_json, serve_op_routes, OpRoute, Plan, Planner, SchemaFn, ServeRunner,
};
use crate::error::{TeleError, TeleResult, EXIT_OK};

pub(crate) struct McpRouteMeta {
    pub(crate) tool_name: String,
    pub(crate) op: &'static str,
    pub(crate) summary: String,
    pub(crate) read_only: bool,
    pub(crate) destructive: bool,
    pub(crate) retry_safe: bool,
}

#[derive(Clone, Copy)]
struct ExecEntry {
    destructive: bool,
    schema_fn: SchemaFn,
    planner: Planner,
    runner: ServeRunner,
    timeout: Option<Duration>,
}

pub(crate) struct TeleMcp {
    shares: ServeShares,
    account: String,
    read_only: bool,
    groups: Option<Vec<String>>,
    routes: Vec<McpRouteMeta>,
    execs: HashMap<&'static str, ExecEntry>,
    visible_names: HashSet<String>,
}

fn tool_name_for_op(op: &str) -> String {
    op.replace(' ', "_")
}

fn group_of(op: &str) -> &str {
    op.split(' ').next().unwrap_or(op)
}

fn build_route_tables(ops: &[OpRoute]) -> (Vec<McpRouteMeta>, HashMap<&'static str, ExecEntry>) {
    let mut metas: Vec<McpRouteMeta> = ops
        .iter()
        .map(|r| McpRouteMeta {
            tool_name: tool_name_for_op(r.op),
            op: r.op,
            summary: r.summary.to_string(),
            read_only: r.read_only,
            destructive: r.destructive,
            retry_safe: r.retry_safe,
        })
        .collect();
    metas.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
    let execs = ops
        .iter()
        .map(|r| {
            (
                r.op,
                ExecEntry {
                    destructive: r.destructive,
                    schema_fn: r.schema_fn,
                    planner: r.planner,
                    runner: r.runner,
                    timeout: r.timeout,
                },
            )
        })
        .collect();
    (metas, execs)
}

fn visible_metas<'a>(
    routes: &'a [McpRouteMeta],
    read_only_gate: bool,
    groups: Option<&[String]>,
) -> Vec<&'a McpRouteMeta> {
    routes
        .iter()
        .filter(|r| !read_only_gate || r.read_only)
        .filter(|r| match groups {
            None => true,
            Some(groups) => groups.iter().any(|g| g == group_of(r.op)),
        })
        .collect()
}

fn destructive_suffix() -> &'static str {
    " Destructive: the first call rejects with ConfirmRequired; resend with arguments.confirm=true to run it."
}

fn tool_for_meta(meta: &McpRouteMeta, exec: &ExecEntry) -> Tool {
    let mut description = meta.summary.clone();
    if meta.destructive {
        description.push_str(destructive_suffix());
    }
    let input_schema = match (exec.schema_fn)() {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let annotations = ToolAnnotations::new()
        .read_only(meta.read_only)
        .destructive(meta.destructive)
        .idempotent(if meta.read_only {
            true
        } else {
            meta.retry_safe
        });
    Tool::new(meta.tool_name.clone(), description, Arc::new(input_schema))
        .with_annotations(annotations)
}

fn text_result(value: serde_json::Value, is_error: bool) -> CallToolResult {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let content = vec![ContentBlock::text(text)];
    if is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

impl TeleMcp {
    pub(crate) fn new(
        shares: ServeShares,
        account: String,
        read_only: bool,
        groups: Option<Vec<String>>,
    ) -> Self {
        let ops = serve_op_routes();
        Self::from_ops(shares, account, read_only, groups, &ops)
    }

    pub(crate) fn from_ops(
        shares: ServeShares,
        account: String,
        read_only: bool,
        mut groups: Option<Vec<String>>,
        ops: &[OpRoute],
    ) -> Self {
        if let Some(list) = groups.as_mut() {
            for g in list.iter_mut() {
                *g = g.trim().to_lowercase();
            }
            list.retain(|g| !g.is_empty());
            if list.is_empty() {
                groups = None;
            }
        }
        let (routes, execs) = build_route_tables(ops);
        let visible_names = visible_metas(&routes, read_only, groups.as_deref())
            .into_iter()
            .filter(|meta| execs.contains_key(meta.op))
            .map(|meta| meta.tool_name.clone())
            .collect();
        Self {
            shares,
            account,
            read_only,
            groups,
            routes,
            execs,
            visible_names,
        }
    }

    fn visible_tools(&self) -> Vec<Tool> {
        visible_metas(&self.routes, self.read_only, self.groups.as_deref())
            .into_iter()
            .filter_map(|meta| {
                let exec = self.execs.get(meta.op)?;
                Some(tool_for_meta(meta, exec))
            })
            .collect()
    }

    fn info(&self) -> ServerInfo {
        let mut instructions = format!(
            "tele MCP server bound to Telegram account \"{}\"; the account is fixed at startup via --account and every tool runs as that account. \
Tools map tele CLI ops one-to-one. Every tool accepts dry_run:true, which returns the would-payload offline without touching the network. \
Destructive tools carry annotations.destructiveHint=true: the first call rejects with a ConfirmRequired error and must be re-sent with arguments.confirm=true on a second call.",
            self.account
        );
        if self.read_only {
            instructions.push_str(
                " This instance was started with --read-only, so mutating tools are hidden.",
            );
        }
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tele", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }

    async fn call_core(
        &self,
        tool_name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.visible_names.contains(tool_name) {
            let visible = self.visible_names.len();
            return Err(ErrorData::invalid_params(
                if self.routes.iter().any(|r| r.tool_name == tool_name) {
                    format!(
                        "tool {tool_name} is hidden by the --read-only or --groups gate on this server; \
use tools/list to see the {visible} available tele tools"
                    )
                } else {
                    format!(
                        "unknown tool {tool_name}; use tools/list to see the {visible} available tele tools"
                    )
                },
                None,
            ));
        }
        let meta = self
            .routes
            .iter()
            .find(|r| r.tool_name == tool_name)
            .expect("visible tool name must exist in route table");
        let Some(exec) = self.execs.get(meta.op).copied() else {
            return Err(ErrorData::internal_error(
                format!("tool {tool_name} has no executor"),
                None,
            ));
        };
        let mut raw = match arguments {
            Some(obj) => serde_json::Value::Object(obj),
            None => serde_json::json!({}),
        };
        if let Err(env) = apply_confirm_gate(meta.op, exec.destructive, exec.planner, &mut raw) {
            return Ok(text_result(env, true));
        }
        match (exec.planner)(meta.op, raw.clone()) {
            Err(env) => Ok(text_result(env, true)),
            Ok(Plan::DryRun(data)) => Ok(text_result(data, false)),
            Ok(Plan::Execute(raw)) => {
                let future = (exec.runner)(self.shares.clone(), raw);
                let outcome = match exec.timeout {
                    Some(limit) => match tokio::time::timeout(limit, future).await {
                        Ok(outcome) => outcome,
                        Err(_) => Err(err_json(
                            "Timeout",
                            format!("op {} timed out after {limit:?}", meta.op),
                        )),
                    },
                    None => future.await,
                };
                match outcome {
                    Ok(data) => Ok(text_result(data, false)),
                    Err(env) => Ok(text_result(env, true)),
                }
            }
        }
    }
}

impl ServerHandler for TeleMcp {
    fn get_info(&self) -> ServerInfo {
        self.info()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.visible_tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.call_core(request.name.as_ref(), request.arguments)
            .await
            .map(CallToolResponse::Complete)
    }
}

#[derive(Parser)]
pub struct McpArgs {
    #[arg(
        long,
        help = "account name owning this MCP session (fixed for its lifetime)"
    )]
    account: String,
    #[arg(long, default_value_t = false, help = "expose only read-only tools")]
    read_only: bool,
    #[arg(
        long,
        value_delimiter = ',',
        help = "keep only these op groups, comma-delimited (e.g. msg,dialog)"
    )]
    groups: Option<Vec<String>>,
}

pub async fn run(args: &McpArgs, flags: &crate::executor::GlobalFlags) -> TeleResult<i32> {
    let name = args.account.trim();
    if name.is_empty() {
        return Err(TeleError::Usage("--account must not be empty".to_string()));
    }
    let known = crate::executor::select_accounts(&crate::executor::GlobalFlags {
        account: vec![name.to_string()],
        ..flags.clone()
    })?;
    if known.len() != 1 || known[0] != name {
        return Err(TeleError::Usage(format!(
            "unknown account '{name}': not in config.toml or no session file"
        )));
    }
    let creds = crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))?;
    let guard = ClientGuard::connect(name, creds.api_id, flags.config_path.as_deref()).await?;
    let outcome = serve_session(&guard, args).await;
    guard.close().await;
    outcome
}

async fn serve_session(guard: &ClientGuard, args: &McpArgs) -> TeleResult<i32> {
    crate::client::authorize(&guard.client).await?;
    let shares = guard.shares();
    let server = TeleMcp::new(
        shares,
        args.account.clone(),
        args.read_only,
        args.groups.clone(),
    );
    output_startup(&server, &args.account);
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| TeleError::Other(format!("mcp: transport init failed: {e}")))?;
    running
        .waiting()
        .await
        .map_err(|e| TeleError::Other(format!("mcp: service task failed: {e}")))?;
    crate::output::log_line("info", "mcp: stdio closed");
    Ok(EXIT_OK)
}

fn output_startup(server: &TeleMcp, account: &str) {
    let mode = if server.read_only {
        "read-only"
    } else {
        "full"
    };
    let visible = visible_metas(&server.routes, server.read_only, server.groups.as_deref()).len();
    crate::output::log_line(
        "info",
        &format!("mcp: serving {visible} tools ({mode}) over stdio for account {account}"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_limiter::RateLimiter;
    use grammers_client::client::ClientConfiguration;
    use grammers_client::sender::ConnectionParams;
    use grammers_client::{Client, SenderPool};
    use rmcp::model::ErrorCode;

    async fn offline_shares(tag: &str) -> ServeShares {
        let dir = std::env::temp_dir().join(format!(
            "telecli-mcp-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let session = Arc::new(
            grammers_client::session::storages::SqliteSession::open(dir.join("s.session"))
                .await
                .unwrap(),
        );
        let pool =
            SenderPool::with_configuration(Arc::clone(&session), 1, ConnectionParams::default());
        let client = Client::with_configuration(pool.handle, ClientConfiguration::default());
        ServeShares {
            client,
            session,
            rate_limiter: RateLimiter::unlimited(),
            _session_lock: None,
        }
    }

    async fn offline_handler(read_only: bool, groups: Option<Vec<String>>) -> TeleMcp {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        TeleMcp::new(
            offline_shares(&format!("h{n}")).await,
            "test-acct".to_string(),
            read_only,
            groups,
        )
    }

    #[test]
    fn tool_names_are_valid_and_unique_across_full_table() {
        let ops = serve_op_routes();
        assert!(!ops.is_empty());
        let re = regex::Regex::new(r"^[A-Za-z0-9_-]{1,128}$").unwrap();
        let mut names: Vec<String> = Vec::new();
        for op in &ops {
            let name = tool_name_for_op(op.op);
            assert!(
                re.is_match(&name),
                "invalid tool name {name:?} from {}",
                op.op
            );
            names.push(name);
        }
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate tool_name in route table");
    }

    #[test]
    fn name_mapping_replaces_spaces_with_underscores() {
        assert_eq!(tool_name_for_op("msg send"), "msg_send");
        assert_eq!(
            tool_name_for_op("account sessions list"),
            "account_sessions_list"
        );
        assert_eq!(tool_name_for_op("raw"), "raw");
        assert_eq!(tool_name_for_op("chat admin-log"), "chat_admin-log");
    }

    #[test]
    fn read_only_gate_drops_every_non_read_only_row() {
        let ops = serve_op_routes();
        let (metas, _) = build_route_tables(&ops);
        let expected = metas.iter().filter(|m| m.read_only).count();
        assert!(expected > 0);
        let visible = visible_metas(&metas, true, None);
        assert_eq!(visible.len(), expected);
        assert!(visible.iter().all(|m| m.read_only));
        assert!(!visible.iter().any(|m| m.tool_name == "msg_send"));
        assert!(visible.iter().any(|m| m.tool_name == "dialog_list"));
        assert_eq!(visible_metas(&metas, false, None).len(), metas.len());
    }

    #[test]
    fn groups_filter_keeps_only_rows_whose_first_token_matches() {
        let ops = serve_op_routes();
        let (metas, _) = build_route_tables(&ops);
        let visible = visible_metas(&metas, false, Some(&["msg".to_string()]));
        assert!(!visible.is_empty());
        assert!(visible.iter().all(|m| m.op.starts_with("msg ")));
        assert!(visible.iter().any(|m| m.tool_name == "msg_send"));

        let both = visible_metas(
            &metas,
            false,
            Some(&["dialog".to_string(), "topic".to_string()]),
        );
        assert!(both
            .iter()
            .all(|m| m.op.starts_with("dialog ") || m.op.starts_with("topic ")));

        let none = visible_metas(&metas, false, Some(&["nosuchgroup".to_string()]));
        assert!(none.is_empty());

        let raw_only = visible_metas(&metas, false, Some(&["raw".to_string()]));
        assert_eq!(raw_only.len(), 1);
        assert_eq!(raw_only[0].tool_name, "raw");
    }

    fn passthrough_planner(_op: &str, raw: serde_json::Value) -> Result<Plan, serde_json::Value> {
        Ok(Plan::Execute(raw))
    }

    fn slow_runner(
        _shares: ServeShares,
        _raw: serde_json::Value,
    ) -> crate::commands::serve::ServeFuture {
        Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            Ok(serde_json::json!({"ok": true}))
        })
    }

    fn slow_route(timeout: Option<std::time::Duration>) -> OpRoute {
        OpRoute {
            op: "test slow",
            lane: crate::commands::serve::Lane::Read,
            timeout,
            read_only: true,
            destructive: false,
            retry_safe: false,
            summary: "slow op used by timeout tests",
            planner: passthrough_planner,
            runner: slow_runner,
            schema_fn: || serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn read_only_gate_rejects_hidden_tools_before_execution() {
        let handler = offline_handler(true, None).await;
        let args = serde_json::json!({"chat": "@game", "all": true});
        let call = handler.call_core("msg_delete", Some(args.as_object().unwrap().clone()));
        let err = tokio::time::timeout(std::time::Duration::from_secs(10), call)
            .await
            .expect("gated tool must be rejected without executing")
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("--read-only"), "{}", err.message);

        let kept_args = serde_json::json!({"dry_run": true});
        let kept = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            handler.call_core("dialog_list", Some(kept_args.as_object().unwrap().clone())),
        )
        .await
        .expect("visible tool must pass the gate");
        assert!(kept.is_ok(), "dialog_list is visible under --read-only");
    }

    #[tokio::test]
    async fn groups_gate_rejects_tools_outside_selected_groups() {
        let handler = offline_handler(false, Some(vec!["dialog".to_string()])).await;
        let args = serde_json::json!({"chat": "@game", "id": 5, "reaction": "+1", "dry_run": true});
        let call = handler.call_core("msg_react", Some(args.as_object().unwrap().clone()));
        let err = tokio::time::timeout(std::time::Duration::from_secs(10), call)
            .await
            .expect("gated tool must be rejected without executing")
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("--groups"), "{}", err.message);

        let kept_args = serde_json::json!({"dry_run": true});
        let kept = handler
            .call_core("dialog_list", Some(kept_args.as_object().unwrap().clone()))
            .await;
        assert!(kept.is_ok(), "dialog_list stays visible for group dialog");
    }

    #[tokio::test]
    async fn unknown_tool_error_reports_visible_count_not_total() {
        let handler = offline_handler(true, None).await;
        let visible = visible_metas(&handler.routes, true, None).len();
        let total = handler.routes.len();
        assert!(visible < total);
        let err = handler
            .call_core("definitely_not_a_tool", None)
            .await
            .unwrap_err();
        let want = format!("the {visible} available");
        let banned = format!("the {total} available");
        assert!(err.message.contains(want.as_str()), "{}", err.message);
        assert!(!err.message.contains(banned.as_str()), "{}", err.message);
    }

    #[tokio::test]
    async fn execute_path_enforces_route_timeout_with_error_envelope() {
        let ops = [slow_route(Some(std::time::Duration::from_millis(50)))];
        let handler = TeleMcp::from_ops(
            offline_shares("timeout").await,
            "test-acct".to_string(),
            false,
            None,
            &ops,
        );
        let started = std::time::Instant::now();
        let result = handler.call_core("test_slow", None).await.unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result.is_error, Some(true));
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "runner await must be bounded, elapsed {elapsed:?}"
        );
        let text = match &result.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            other => panic!("expected text content, got {other:?}"),
        };
        assert!(text.contains("\"Timeout\""), "{text}");
        assert!(text.contains("timed out after"), "{text}");

        let untimed = [slow_route(None)];
        let loose = TeleMcp::from_ops(
            offline_shares("timeout-none").await,
            "test-acct".to_string(),
            false,
            None,
            &untimed,
        );
        let ok = loose.call_core("test_slow", None).await.unwrap();
        assert_eq!(ok.is_error, Some(false));
    }

    #[tokio::test]
    async fn unknown_tool_returns_invalid_params_error() {
        let handler = offline_handler(false, None).await;
        let err = handler
            .call_core("definitely_not_a_tool", None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("unknown tool"), "{}", err.message);

        let dotted = handler.call_core("msg.send", None).await.unwrap_err();
        assert_eq!(dotted.code, ErrorCode::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn destructive_without_confirm_yields_is_error_text_containing_confirm_required() {
        let handler = offline_handler(false, None).await;
        let args = serde_json::json!({"chat": "@game", "all": true});
        let result = handler
            .call_core("msg_delete", Some(args.as_object().unwrap().clone()))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = match &result.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            other => panic!("expected text content, got {other:?}"),
        };
        assert!(text.contains("ConfirmRequired"), "{text}");
        assert!(text.contains("would"), "{text}");
        assert!(text.contains("delete all messages in chat @game"), "{text}");
    }

    #[tokio::test]
    async fn dry_run_roundtrip_succeeds_offline_with_pretty_would_payload() {
        let handler = offline_handler(false, None).await;
        let args = serde_json::json!({"chat": "@game", "id": 5, "reaction": "+1", "dry_run": true});
        let result = handler
            .call_core("msg_react", Some(args.as_object().unwrap().clone()))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let text = match &result.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            other => panic!("expected text content, got {other:?}"),
        };
        assert!(text.contains('\n'), "expected pretty JSON: {text}");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["id"], 5);
        assert_eq!(parsed["would"], "react +1 to message 5");
    }

    #[tokio::test]
    async fn planner_error_envelope_becomes_error_text_result() {
        let handler = offline_handler(false, None).await;
        let args = serde_json::json!({"chat": "@x"});
        let result = handler
            .call_core("msg_edit", Some(args.as_object().unwrap().clone()))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = match &result.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            other => panic!("expected text content, got {other:?}"),
        };
        assert!(text.contains("missing field"), "{text}");

        let missing_args = handler.call_core("msg_edit", None).await.unwrap();
        assert_eq!(missing_args.is_error, Some(true));
    }

    #[test]
    fn list_tools_filters_and_annotates_without_context() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handler = rt.block_on(offline_handler(false, Some(vec!["msg".to_string()])));
        let tools = handler.visible_tools();
        assert!(!tools.is_empty());
        assert!(tools.iter().all(|t| t.name.starts_with("msg_")));

        let by_name: HashMap<&str, &Tool> = tools.iter().map(|t| (t.name.as_ref(), t)).collect();
        let send = by_name["msg_send"];
        let annotations = send.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(false));

        let get = by_name["msg_get"];
        let annotations = get.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert!(!get
            .description
            .as_ref()
            .unwrap()
            .contains("ConfirmRequired"));

        let delete = by_name["msg_delete"];
        assert_eq!(
            delete.annotations.as_ref().unwrap().destructive_hint,
            Some(true)
        );
        assert!(delete
            .description
            .as_ref()
            .unwrap()
            .contains("arguments.confirm=true"));
    }

    #[test]
    fn get_info_declares_tele_identity_tools_and_instructions() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let handler = rt.block_on(offline_handler(true, None));
        let info = handler.info();
        assert_eq!(info.server_info.name, "tele");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        assert!(
            info.capabilities.tools.is_some(),
            "tools capability required"
        );
        let instructions = info.instructions.as_ref().unwrap();
        assert!(instructions.contains("--account"), "{instructions}");
        assert!(instructions.contains("dry_run:true"), "{instructions}");
        assert!(instructions.contains("ConfirmRequired"), "{instructions}");
        assert!(instructions.contains("confirm=true"), "{instructions}");
        assert!(instructions.contains("read-only"), "{instructions}");

        let full = rt.block_on(offline_handler(false, None));
        let instructions = full.info().instructions.unwrap();
        assert!(!instructions.contains("read-only"), "{instructions}");
    }
}
