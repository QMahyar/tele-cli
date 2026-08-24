use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::entities;
use crate::error::tele_invocation;
use crate::error::TeleError;
use crate::error::TeleResult;
use crate::executor::{run_fanout, GlobalFlags};
use crate::output;

#[derive(Subcommand)]
pub enum ContactCmd {
    List(ListArgs),
    Add(AddArgs),
    Remove(RemoveArgs),
    Block(BlockArgs),
    Unblock(BlockArgs),
}

#[derive(Args, Clone)]
pub struct ListArgs {
    #[arg(long, default_value_t = 100, help = "max contacts to list (1-10000)")]
    limit: u32,
}

#[derive(Args, Clone)]
pub struct AddArgs {
    #[arg(long, help = "user to add: @username, numeric ID, +phone, or me")]
    user: String,
    #[arg(long, help = "first name (defaults to peer name)")]
    first: Option<String>,
    #[arg(long, help = "last name (defaults to peer name)")]
    last: Option<String>,
    #[arg(long, help = "phone number to associate")]
    phone: Option<String>,
}

#[derive(Args, Clone)]
pub struct RemoveArgs {
    #[arg(
        long,
        help = "user to remove from contacts: @username, numeric ID, +phone, or me"
    )]
    user: String,
}

#[derive(Args, Clone)]
pub struct BlockArgs {
    #[arg(
        long,
        help = "user to block/unblock: @username, numeric ID, +phone, or me"
    )]
    user: String,
}

pub async fn run(cmd: ContactCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        ContactCmd::List(a) => list(a, flags).await,
        ContactCmd::Add(a) => add(a, flags).await,
        ContactCmd::Remove(a) => remove(a, flags).await,
        ContactCmd::Block(a) => block(a, flags).await,
        ContactCmd::Unblock(a) => unblock(a, flags).await,
    }
}

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_list(&args)?;
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
                return list_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            let result = list_core(&guard.shares(), ListParams::from(&args)).await?;
            if !output::machine_mode(json, jsonl) {
                let empty = Vec::new();
                let table_rows: Vec<Vec<String>> = result["contacts"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(contact_table_row)
                    .collect();
                output::print_account_table(
                    &name,
                    multi,
                    &["id", "name", "phone", "username"],
                    &table_rows,
                )?;
            }
            Ok(result)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn add(args: AddArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_add(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return add_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            add_core(&guard.shares(), AddParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn remove(args: RemoveArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_remove(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return remove_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            remove_core(&guard.shares(), RemoveParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn block(args: BlockArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_block(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return block_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            block_core(&guard.shares(), BlockParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn unblock(args: BlockArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_block(&args)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return unblock_serve_dry_run(&args);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            unblock_core(&guard.shares(), BlockParams::from(&args)).await
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

fn dry_run_payload(action: &str, user: Option<&str>) -> serde_json::Value {
    match user {
        Some(u) => serde_json::json!({
            "dry_run": true,
            "user": u,
            "would": format!("{action} user {u}")
        }),
        None => serde_json::json!({
            "dry_run": true,
            "would": action
        }),
    }
}

type ContactState = (bool, bool, String);

fn returned_user_state(updates: &tl::enums::Updates) -> Option<ContactState> {
    let users: &[tl::enums::User] = match updates {
        tl::enums::Updates::Updates(u) => &u.users,
        tl::enums::Updates::Combined(u) => &u.users,
        _ => return None,
    };
    let user = users.first()?;
    match user {
        tl::enums::User::User(u) => {
            let name = format!(
                "{} {}",
                u.first_name.as_deref().unwrap_or_default(),
                u.last_name.as_deref().unwrap_or_default()
            )
            .trim()
            .to_string();
            Some((u.contact, u.mutual_contact, name))
        }
        tl::enums::User::Empty(_) => None,
    }
}

fn sent_display_name(first: &str, last: &str) -> String {
    format!("{first} {last}").trim().to_string()
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    #[serde(default = "default_contact_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

fn default_contact_limit() -> u32 {
    100
}

impl From<&ListArgs> for ListParams {
    fn from(a: &ListArgs) -> Self {
        Self {
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&ListParams> for ListArgs {
    fn from(p: &ListParams) -> Self {
        Self { limit: p.limit }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddParams {
    pub(crate) user: String,
    pub(crate) first: Option<String>,
    pub(crate) last: Option<String>,
    pub(crate) phone: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&AddArgs> for AddParams {
    fn from(a: &AddArgs) -> Self {
        Self {
            user: a.user.clone(),
            first: a.first.clone(),
            last: a.last.clone(),
            phone: a.phone.clone(),
            dry_run: false,
        }
    }
}

impl From<&AddParams> for AddArgs {
    fn from(p: &AddParams) -> Self {
        Self {
            user: p.user.clone(),
            first: p.first.clone(),
            last: p.last.clone(),
            phone: p.phone.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoveParams {
    pub(crate) user: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&RemoveArgs> for RemoveParams {
    fn from(a: &RemoveArgs) -> Self {
        Self {
            user: a.user.clone(),
            dry_run: false,
        }
    }
}

impl From<&RemoveParams> for RemoveArgs {
    fn from(p: &RemoveParams) -> Self {
        Self {
            user: p.user.clone(),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlockParams {
    pub(crate) user: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&BlockArgs> for BlockParams {
    fn from(a: &BlockArgs) -> Self {
        Self {
            user: a.user.clone(),
            dry_run: false,
        }
    }
}

impl From<&BlockParams> for BlockArgs {
    fn from(p: &BlockParams) -> Self {
        Self {
            user: p.user.clone(),
        }
    }
}

fn validate_list(args: &ListArgs) -> TeleResult<()> {
    crate::commands::validate_limit(args.limit, 10_000, "limit").map(|_| ())
}

fn require_contact_user(op: &str, user: &str) -> TeleResult<()> {
    crate::commands::require_chat_target(user, &format!("contact {op} --user"))
}

fn validate_add(args: &AddArgs) -> TeleResult<()> {
    require_contact_user("add", &args.user)
}

fn validate_remove(args: &RemoveArgs) -> TeleResult<()> {
    require_contact_user("remove", &args.user)
}

fn validate_block(args: &BlockArgs) -> TeleResult<()> {
    require_contact_user("block", &args.user)
}

fn list_serve_dry_run(_args: &ListArgs) -> TeleResult<serde_json::Value> {
    Ok(dry_run_payload("list contacts", None))
}

fn add_serve_dry_run(args: &AddArgs) -> TeleResult<serde_json::Value> {
    Ok(dry_run_payload("add contact", Some(&args.user)))
}

fn remove_serve_dry_run(args: &RemoveArgs) -> TeleResult<serde_json::Value> {
    Ok(dry_run_payload("remove from contacts", Some(&args.user)))
}

fn block_serve_dry_run(args: &BlockArgs) -> TeleResult<serde_json::Value> {
    Ok(dry_run_payload("block", Some(&args.user)))
}

fn unblock_serve_dry_run(args: &BlockArgs) -> TeleResult<serde_json::Value> {
    Ok(dry_run_payload("unblock", Some(&args.user)))
}

fn contact_table_row(row: &serde_json::Value) -> Vec<String> {
    vec![
        row["id"].to_string(),
        row["name"].as_str().unwrap_or_default().to_string(),
        row["phone"].as_str().unwrap_or_default().to_string(),
        row["username"].as_str().unwrap_or_default().to_string(),
    ]
}

pub(crate) async fn list_core(
    shares: &crate::client::ServeShares,
    params: ListParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let raw: tl::enums::contacts::Contacts = shares
        .client
        .invoke(&tl::functions::contacts::GetContacts { hash: 0 })
        .await
        .map_err(tele_invocation)?;
    let users = match raw {
        tl::enums::contacts::Contacts::NotModified => Vec::new(),
        tl::enums::contacts::Contacts::Contacts(contacts) => contacts.users,
    };
    let mut rows = Vec::new();
    for user in users.into_iter().take(params.limit as usize) {
        if let tl::enums::User::User(user) = user {
            rows.push(serde_json::json!({
                "id": user.id,
                "name": format!(
                    "{} {}",
                    user.first_name.clone().unwrap_or_default(),
                    user.last_name.clone().unwrap_or_default()
                ).trim().to_string(),
                "phone": user.phone.as_deref().unwrap_or_default(),
                "username": user.username.as_deref().unwrap_or_default(),
            }));
        }
    }
    Ok(serde_json::json!({"contacts": rows}))
}

pub(crate) async fn add_core(
    shares: &crate::client::ServeShares,
    params: AddParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let user_target = params.user.clone();
    let peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &user_target).await?;
    let user_input = entities::input_user(&peer).await.map_err(tele_invocation)?;
    let peer_name = crate::serialize::peer_name(&peer);
    let (f, l) = match (params.first, params.last) {
        (Some(f), Some(l)) => (f, l),
        _ => {
            let mut parts = peer_name.splitn(2, ' ');
            (
                parts.next().unwrap_or(&peer_name).to_string(),
                parts.next().unwrap_or("").to_string(),
            )
        }
    };
    let updates: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::contacts::AddContact {
            add_phone_privacy_exception: false,
            id: user_input,
            first_name: f.clone(),
            last_name: l.clone(),
            phone: params.phone.unwrap_or_default(),
            note: None,
        })
        .await
        .map_err(tele_invocation)?;
    let Some((contact, mutual, server_name)) = returned_user_state(&updates) else {
        return Err(TeleError::Other(format!(
            "contact not added: {user_target}'s privacy settings do not allow adding your number"
        )));
    };
    if !contact {
        return Err(TeleError::Other(format!(
            "contact not saved to your contact list (privacy settings of {user_target})"
        )));
    }
    let sent = sent_display_name(&f, &l);
    if !sent.is_empty() && !server_name.is_empty() && server_name != sent {
        crate::output::log_line(
            "warn",
            "contact already existed; its display name was updated to the new values",
        );
    }
    Ok(serde_json::json!({
        "user": user_target,
        "added": true,
        "contact": contact,
        "mutual": mutual,
    }))
}

pub(crate) async fn remove_core(
    shares: &crate::client::ServeShares,
    params: RemoveParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let user_target = params.user.clone();
    let peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &user_target).await?;
    if matches!(
        peer,
        grammers_client::peer::Peer::Group(_) | grammers_client::peer::Peer::Channel(_)
    ) {
        return Err(TeleError::Usage(
            "contact remove targets a user; got a chat or channel".to_string(),
        ));
    }
    let user_input = entities::input_user(&peer).await.map_err(tele_invocation)?;
    let _: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::contacts::DeleteContacts {
            id: vec![user_input],
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({"user": user_target, "removed": true}))
}

pub(crate) async fn block_core(
    shares: &crate::client::ServeShares,
    params: BlockParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let user_target = params.user.clone();
    let peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &user_target).await?;
    let input = entities::input_peer(&peer).await.map_err(tele_invocation)?;
    let _: bool = shares
        .client
        .invoke(&tl::functions::contacts::Block {
            my_stories_from: false,
            id: input,
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({"user": user_target, "blocked": true}))
}

pub(crate) async fn unblock_core(
    shares: &crate::client::ServeShares,
    params: BlockParams,
) -> TeleResult<serde_json::Value> {
    shares.rate_limiter.acquire().await;
    let user_target = params.user.clone();
    let peer =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &user_target).await?;
    let input = entities::input_peer(&peer).await.map_err(tele_invocation)?;
    let _: bool = shares
        .client
        .invoke(&tl::functions::contacts::Unblock {
            my_stories_from: false,
            id: input,
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({"user": user_target, "blocked": false}))
}

pub(crate) fn contact_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
    vec![
        crate::serve_route!(
            "contact add",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "add a contact by phone number",
            AddParams,
            AddArgs,
            validate_add,
            add_serve_dry_run,
            run_add
        ),
        crate::serve_route!(
            "contact block",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "block a user",
            BlockParams,
            BlockArgs,
            validate_block,
            block_serve_dry_run,
            run_block
        ),
        crate::serve_route!(
            "contact list",
            Lane::Read,
            Some(OP_TIMEOUT_PAGINATED),
            true,
            false,
            true,
            "list contacts",
            ListParams,
            ListArgs,
            validate_list,
            list_serve_dry_run,
            run_list
        ),
        crate::serve_route!(
            "contact remove",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            true,
            true,
            "remove a contact",
            RemoveParams,
            RemoveArgs,
            validate_remove,
            remove_serve_dry_run,
            run_remove
        ),
        crate::serve_route!(
            "contact unblock",
            Lane::Mutate,
            Some(OP_TIMEOUT_SIMPLE),
            false,
            false,
            true,
            "unblock a user",
            BlockParams,
            BlockArgs,
            validate_block,
            unblock_serve_dry_run,
            run_unblock
        ),
    ]
}

crate::serve_runner!(run_list, list_core, ListParams);
crate::serve_runner!(run_add, add_core, AddParams);
crate::serve_runner!(run_remove, remove_core, RemoveParams);
crate::serve_runner!(run_block, block_core, BlockParams);
crate::serve_runner!(run_unblock, unblock_core, BlockParams);

#[cfg(test)]
mod tests {
    use super::*;

    fn contact_added_updates(
        contact: bool,
        mutual: bool,
        first: &str,
        last: &str,
    ) -> tl::enums::Updates {
        tl::enums::Updates::Updates(tl::types::Updates {
            updates: Vec::new(),
            users: vec![tl::enums::User::User(tl::types::User {
                is_self: false,
                contact,
                mutual_contact: mutual,
                deleted: false,
                bot: false,
                bot_chat_history: false,
                bot_nochats: false,
                verified: false,
                restricted: false,
                min: false,
                bot_inline_geo: false,
                support: false,
                scam: false,
                apply_min_photo: false,
                fake: false,
                bot_attach_menu: false,
                premium: false,
                attach_menu_enabled: false,
                bot_can_edit: false,
                close_friend: false,
                stories_hidden: false,
                stories_unavailable: false,
                contact_require_premium: false,
                bot_business: false,
                bot_has_main_app: false,
                bot_forum_view: false,
                bot_forum_can_manage_topics: false,
                bot_can_manage_bots: false,
                bot_guestchat: false,
                bot_guard: false,
                id: 4242,
                access_hash: Some(777),
                first_name: Some(first.to_string()),
                last_name: Some(last.to_string()),
                username: None,
                phone: None,
                photo: None,
                status: None,
                bot_info_version: None,
                restriction_reason: None,
                bot_inline_placeholder: None,
                lang_code: None,
                emoji_status: None,
                usernames: None,
                stories_max_id: None,
                color: None,
                profile_color: None,
                bot_active_users: None,
                bot_verification_icon: None,
                send_paid_messages_stars: None,
            })],
            chats: Vec::new(),
            date: 0,
            seq: 0,
        })
    }
    #[test]
    fn returned_user_state_reads_contact_flags() {
        let updates = contact_added_updates(true, true, "Jane", "Doe");
        let state = returned_user_state(&updates).expect("user present");
        assert!(state.0);
        assert!(state.1);
        assert_eq!(state.2, "Jane Doe");
    }

    #[test]
    fn returned_user_state_flags_blocked_add_as_not_contact() {
        let updates = contact_added_updates(false, false, "Jane", "Doe");
        let state = returned_user_state(&updates).expect("user present");
        assert!(!state.0);
        assert!(!state.1);
    }

    #[test]
    fn returned_user_state_none_without_users_payload() {
        let combined = tl::enums::Updates::Combined(tl::types::UpdatesCombined {
            updates: Vec::new(),
            users: Vec::new(),
            chats: Vec::new(),
            date: 0,
            seq_start: 0,
            seq: 0,
        });
        assert!(returned_user_state(&combined).is_none());
    }

    #[test]
    fn sent_display_name_trims_and_joins() {
        assert_eq!(sent_display_name("Jane", "Doe"), "Jane Doe");
        assert_eq!(sent_display_name("Jane", ""), "Jane");
        assert_eq!(sent_display_name("", ""), "");
    }

    #[test]
    fn dry_run_payload_marks_dry_run_only() {
        let v = dry_run_payload("list contacts", None);
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert!(v.get("user").is_none());
        assert_eq!(v["would"], serde_json::json!("list contacts"));
    }

    #[test]
    fn dry_run_payload_carries_user_target() {
        let v = dry_run_payload("block", Some("alice"));
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["user"], serde_json::json!("alice"));
        assert_eq!(v["would"], serde_json::json!("block user alice"));
    }

    #[test]
    fn cli_parses_remove_subcommand() {
        use clap::Parser;
        let parsed = crate::Cli::try_parse_from(["tele", "contact", "remove", "--user", "@alice"]);
        match parsed {
            Ok(cli) => {
                let crate::Command::Contact(ContactCmd::Remove(args)) = cli.command else {
                    panic!("expected contact remove");
                };
                assert_eq!(args.user, "@alice");
            }
            Err(e) => panic!("contact remove failed to parse: {e}"),
        }
    }

    #[test]
    fn dry_run_remove_carries_user() {
        let v = dry_run_payload("remove from contacts", Some("@alice"));
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["user"], serde_json::json!("@alice"));
        assert_eq!(
            v["would"],
            serde_json::json!("remove from contacts user @alice")
        );
    }

    fn plan_for(
        op: &str,
        params: serde_json::Value,
    ) -> Result<crate::commands::serve::Plan, serde_json::Value> {
        let routes = contact_serve_routes();
        let route = routes
            .iter()
            .find(|r| r.op == op)
            .unwrap_or_else(|| panic!("route missing for {op}"));
        (route.planner)(op, params)
    }

    #[test]
    fn serve_routes_declare_lanes_and_timeouts() {
        use crate::commands::serve::{Lane, OP_TIMEOUT_PAGINATED, OP_TIMEOUT_SIMPLE};
        let routes = contact_serve_routes();
        let want: Vec<(&str, Lane, Option<std::time::Duration>)> = vec![
            ("contact add", Lane::Mutate, Some(OP_TIMEOUT_SIMPLE)),
            ("contact block", Lane::Mutate, Some(OP_TIMEOUT_SIMPLE)),
            ("contact list", Lane::Read, Some(OP_TIMEOUT_PAGINATED)),
            ("contact remove", Lane::Mutate, Some(OP_TIMEOUT_SIMPLE)),
            ("contact unblock", Lane::Mutate, Some(OP_TIMEOUT_SIMPLE)),
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
    fn serve_missing_required_field_yields_serve_error() {
        for op in [
            "contact add",
            "contact remove",
            "contact block",
            "contact unblock",
        ] {
            let err = plan_for(op, serde_json::json!({})).unwrap_err();
            assert_eq!(err["type"], "ServeError", "{op}");
            let msg = err["message"].as_str().unwrap();
            assert!(msg.contains(op), "{op}: {msg}");
            assert!(msg.contains("user"), "{op}: {msg}");
            assert!(msg.contains("missing field"), "{op}: {msg}");
        }
    }

    #[test]
    fn serve_wrong_type_param_yields_serve_error() {
        for (op, params, fragment) in [
            ("contact list", serde_json::json!({"limit": "many"}), "u32"),
            (
                "contact add",
                serde_json::json!({"user": 42}),
                "expected a string",
            ),
            (
                "contact add",
                serde_json::json!({"user": "@a", "first": 7}),
                "expected a string",
            ),
            (
                "contact add",
                serde_json::json!({"user": "@a", "phone": 1555}),
                "expected a string",
            ),
            (
                "contact remove",
                serde_json::json!({"user": true}),
                "expected a string",
            ),
            (
                "contact block",
                serde_json::json!({"user": []}),
                "expected a string",
            ),
        ] {
            let err = plan_for(op, params).unwrap_err();
            assert_eq!(err["type"], "ServeError", "{op}");
            let msg = err["message"].as_str().unwrap();
            assert!(msg.contains(fragment), "{op}: {msg}");
        }
    }

    #[test]
    fn serve_unknown_param_yields_serve_error() {
        let err = plan_for(
            "contact add",
            serde_json::json!({"user": "@a", "userr": "typo"}),
        )
        .unwrap_err();
        assert_eq!(err["type"], "ServeError");
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("unknown field"), "{msg}");
        assert!(msg.contains("userr"), "{msg}");
    }

    #[test]
    fn serve_validation_usage_errors_stay_pure() {
        for op in [
            "contact add",
            "contact remove",
            "contact block",
            "contact unblock",
        ] {
            for blank in ["", "   "] {
                let params = serde_json::json!({ "user": blank });
                let err = plan_for(op, params).unwrap_err();
                assert_eq!(err["type"], "UsageError", "{op}: {blank:?}");
                assert!(err["message"].as_str().unwrap().contains("--user"), "{op}");
            }
        }
        let err = plan_for("contact list", serde_json::json!({"limit": 10001})).unwrap_err();
        assert_eq!(err["type"], "UsageError");
        assert!(err["message"].as_str().unwrap().contains("--limit"));

        assert!(plan_for("contact list", serde_json::json!({})).is_ok());
        assert!(plan_for("contact list", serde_json::json!({"limit": 10_000})).is_ok());
    }

    #[test]
    fn serve_dry_run_payloads_match_cli_shapes() {
        let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
            (
                "contact list",
                serde_json::json!({"dry_run": true}),
                serde_json::json!({"dry_run": true, "would": "list contacts"}),
            ),
            (
                "contact add",
                serde_json::json!({"user": "@alice", "dry_run": true}),
                serde_json::json!({
                    "dry_run": true,
                    "user": "@alice",
                    "would": "add contact user @alice"
                }),
            ),
            (
                "contact remove",
                serde_json::json!({"user": "+989121234567", "dry_run": true}),
                serde_json::json!({
                    "dry_run": true,
                    "user": "+989121234567",
                    "would": "remove from contacts user +989121234567"
                }),
            ),
            (
                "contact block",
                serde_json::json!({"user": "@bob", "dry_run": true}),
                serde_json::json!({
                    "dry_run": true,
                    "user": "@bob",
                    "would": "block user @bob"
                }),
            ),
            (
                "contact unblock",
                serde_json::json!({"user": "@bob", "dry_run": true}),
                serde_json::json!({
                    "dry_run": true,
                    "user": "@bob",
                    "would": "unblock user @bob"
                }),
            ),
        ];
        for (op, params, want) in cases {
            let plan = plan_for(op, params).unwrap();
            let crate::commands::serve::Plan::DryRun(v) = plan else {
                panic!("expected dry run plan for {op}")
            };
            assert_eq!(v, want, "{op}");
        }
    }

    #[test]
    fn serve_execute_plan_passes_raw_params_through() {
        for (op, raw) in [
            ("contact list", serde_json::json!({})),
            ("contact list", serde_json::json!({"limit": 50})),
            (
                "contact add",
                serde_json::json!({"user": "@alice", "first": "Alice"}),
            ),
            ("contact remove", serde_json::json!({"user": "4242"})),
            ("contact block", serde_json::json!({"user": "@spam"})),
            ("contact unblock", serde_json::json!({"user": "@spam"})),
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
