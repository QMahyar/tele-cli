use clap::Args;
use grammers_client::tl;

use crate::commands::helpers::peer_id;
use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};

#[derive(Args, Clone)]
pub struct PollCloseArgs {
    #[arg(long, value_parser = clap::value_parser!(crate::chat_target::ChatTarget), help = "target chat: @username, t.me link, numeric ID, +phone, or me")]
    pub(crate) chat: crate::chat_target::ChatTarget,
    #[arg(long, help = "message ID of the poll")]
    pub(crate) id: i32,
}

#[derive(Args, Clone)]
pub struct PollResultsArgs {
    #[arg(long, value_parser = clap::value_parser!(crate::chat_target::ChatTarget), help = "target chat: @username, t.me link, numeric ID, +phone, or me")]
    pub(crate) chat: crate::chat_target::ChatTarget,
    #[arg(long, help = "message ID of the poll")]
    pub(crate) id: i32,
}

#[derive(Args, Clone)]
pub struct PollVotesArgs {
    #[arg(long, value_parser = clap::value_parser!(crate::chat_target::ChatTarget), help = "target chat: @username, t.me link, numeric ID, +phone, or me")]
    pub(crate) chat: crate::chat_target::ChatTarget,
    #[arg(long, help = "message ID of the poll")]
    pub(crate) id: i32,
    #[arg(long, help = "1-based option index to list voters for (omit for all)")]
    pub(crate) option: Option<usize>,
    #[arg(long, default_value_t = 20, help = "max voters to return (1-100)")]
    pub(crate) limit: u32,
    #[arg(long, default_value = "", help = "paging offset returned by a previous call")]
    pub(crate) offset: String,
}

#[derive(Args, Clone)]
pub struct PollUnreadArgs {
    #[arg(long, value_parser = clap::value_parser!(crate::chat_target::ChatTarget), help = "target chat: @username, t.me link, numeric ID, +phone, or me")]
    pub(crate) chat: crate::chat_target::ChatTarget,
    #[arg(long, help = "forum topic root message id to scope the listing")]
    pub(crate) top: Option<i32>,
    #[arg(long, default_value_t = 10, help = "max messages to return (1-100)")]
    pub(crate) limit: u32,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct PollCloseParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct PollResultsParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct PollVotesParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    pub(crate) option: Option<usize>,
    #[serde(default = "default_votes_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) offset: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct PollUnreadParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) top: Option<i32>,
    #[serde(default = "default_unread_limit")]
    pub(crate) limit: u32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

fn default_votes_limit() -> u32 {
    20
}

fn default_unread_limit() -> u32 {
    10
}

impl From<&PollCloseArgs> for PollCloseParams {
    fn from(a: &PollCloseArgs) -> Self {
        Self {
            chat: a.chat.as_str().to_string(),
            id: a.id,
            dry_run: false,
        }
    }
}

impl From<&PollCloseParams> for PollCloseArgs {
    fn from(p: &PollCloseParams) -> Self {
        Self {
            chat: crate::chat_target::ChatTarget::new_unchecked(p.chat.clone()),
            id: p.id,
        }
    }
}

impl From<&PollResultsArgs> for PollResultsParams {
    fn from(a: &PollResultsArgs) -> Self {
        Self {
            chat: a.chat.as_str().to_string(),
            id: a.id,
            dry_run: false,
        }
    }
}

impl From<&PollResultsParams> for PollResultsArgs {
    fn from(p: &PollResultsParams) -> Self {
        Self {
            chat: crate::chat_target::ChatTarget::new_unchecked(p.chat.clone()),
            id: p.id,
        }
    }
}

impl From<&PollVotesArgs> for PollVotesParams {
    fn from(a: &PollVotesArgs) -> Self {
        Self {
            chat: a.chat.as_str().to_string(),
            id: a.id,
            option: a.option,
            limit: a.limit,
            offset: a.offset.clone(),
            dry_run: false,
        }
    }
}

impl From<&PollVotesParams> for PollVotesArgs {
    fn from(p: &PollVotesParams) -> Self {
        Self {
            chat: crate::chat_target::ChatTarget::new_unchecked(p.chat.clone()),
            id: p.id,
            option: p.option,
            limit: p.limit,
            offset: p.offset.clone(),
        }
    }
}

impl From<&PollUnreadArgs> for PollUnreadParams {
    fn from(a: &PollUnreadArgs) -> Self {
        Self {
            chat: a.chat.as_str().to_string(),
            top: a.top,
            limit: a.limit,
            dry_run: false,
        }
    }
}

impl From<&PollUnreadParams> for PollUnreadArgs {
    fn from(p: &PollUnreadParams) -> Self {
        Self {
            chat: crate::chat_target::ChatTarget::new_unchecked(p.chat.clone()),
            top: p.top,
            limit: p.limit,
        }
    }
}

pub(crate) fn validate_poll_close(args: &PollCloseArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_poll_results(args: &PollResultsArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_poll_votes(args: &PollVotesArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    if let Some(o) = args.option {
        if o == 0 {
            return Err(TeleError::Usage(
                "--option indexes are 1-based (got 0)".to_string(),
            ));
        }
    }
    crate::commands::validate_limit(args.limit, 100, "limit")?;
    Ok(())
}

pub(crate) fn validate_poll_unread(args: &PollUnreadArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if let Some(t) = args.top {
        if t <= 0 {
            return Err(TeleError::Usage(
                "--top must be a positive topic ID".to_string(),
            ));
        }
    }
    crate::commands::validate_limit(args.limit, 100, "limit")?;
    Ok(())
}

pub(crate) fn poll_close_serve_dry_run(args: &PollCloseArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat.as_str(),
        "id": args.id,
        "would": format!("close poll in message {} in chat {}", args.id, args.chat.as_str())}))
}

pub(crate) fn poll_results_serve_dry_run(
    args: &PollResultsArgs,
) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat.as_str(),
        "id": args.id,
        "would": format!("fetch poll results for message {} in chat {}", args.id, args.chat.as_str())}))
}

pub(crate) fn poll_votes_serve_dry_run(args: &PollVotesArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat.as_str(),
        "id": args.id,
        "option": args.option,
        "limit": args.limit,
        "would": format!("list poll voters for message {} in chat {}", args.id, args.chat.as_str())}))
}

pub(crate) fn poll_unread_serve_dry_run(args: &PollUnreadArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat.as_str(),
        "top": args.top,
        "limit": args.limit,
        "would": format!("list messages with unread poll votes in chat {}", args.chat.as_str())}))
}

fn text_of(t: &tl::enums::TextWithEntities) -> String {
    match t {
        tl::enums::TextWithEntities::Entities(t) => t.text.clone(),
    }
}

async fn fetch_poll(
    shares: &crate::client::ServeShares,
    chat_target: &str,
    id: i32,
) -> TeleResult<(grammers_client::peer::Peer, grammers_client::media::Poll)> {
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), chat_target).await?;
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
    Ok((chat, poll))
}

pub(crate) async fn poll_close_core(
    shares: &crate::client::ServeShares,
    params: PollCloseParams,
) -> TeleResult<serde_json::Value> {
    if params.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    let (chat, poll) = fetch_poll(shares, &params.chat, params.id).await?;
    if poll.closed() {
        return Ok(serde_json::json!({"id": params.id, "closed": true, "already": true}));
    }
    let mut raw_poll = poll.raw.clone();
    raw_poll.closed = true;
    let input_poll = tl::types::InputMediaPoll {
        poll: tl::enums::Poll::Poll(raw_poll),
        correct_answers: None,
        solution: None,
        solution_entities: None,
        attached_media: None,
        solution_media: None,
    };
    let media: tl::enums::InputMedia = input_poll.into();
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    shares.rate_limiter.acquire().await;
    shares
        .client
        .invoke(&tl::functions::messages::EditMessage {
            no_webpage: false,
            invert_media: false,
            peer,
            id: params.id,
            message: None,
            media: Some(media),
            reply_markup: None,
            entities: None,
            schedule_date: None,
            schedule_repeat_period: None,
            quick_reply_shortcut_id: None,
            rich_message: None,
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({"id": params.id, "closed": true}))
}

fn fresh_poll_row(
    id: i32,
    poll: &tl::types::Poll,
    results: &tl::types::PollResults,
) -> serde_json::Value {
    let options: Vec<serde_json::Value> = poll
        .answers
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let (text, option) = match a {
                tl::enums::PollAnswer::Answer(a) => (text_of(&a.text), a.option.clone()),
                tl::enums::PollAnswer::InputPollAnswer(_) => (String::new(), Vec::new()),
            };
            let mut entry = serde_json::Map::new();
            entry.insert("index".into(), serde_json::json!(i + 1));
            entry.insert("text".into(), serde_json::json!(text));
            if let Some(list) = results.results.as_ref() {
                if let Some(v) = list.iter().find_map(|v| match v {
                    tl::enums::PollAnswerVoters::Voters(v) if v.option == option => Some(v),
                    _ => None,
                }) {
                    entry.insert("chosen".into(), serde_json::json!(v.chosen));
                    if v.correct {
                        entry.insert("correct".into(), serde_json::json!(true));
                    }
                    if let Some(count) = v.voters {
                        entry.insert("voters".into(), serde_json::json!(count));
                    }
                }
            }
            serde_json::Value::Object(entry)
        })
        .collect();
    let mut out = serde_json::Map::new();
    out.insert("id".into(), serde_json::json!(id));
    out.insert("poll_id".into(), serde_json::json!(poll.id));
    out.insert(
        "question".into(),
        serde_json::json!(text_of(&poll.question)),
    );
    out.insert("closed".into(), serde_json::json!(poll.closed));
    out.insert("quiz".into(), serde_json::json!(poll.quiz));
    if let Some(total) = results.total_voters {
        out.insert("total_voters".into(), serde_json::json!(total));
    }
    out.insert(
        "has_unread_votes".into(),
        serde_json::json!(results.has_unread_votes),
    );
    out.insert("options".into(), serde_json::Value::Array(options));
    serde_json::Value::Object(out)
}

fn updates_poll_parts(
    updates: &tl::enums::Updates,
) -> Option<(tl::types::Poll, tl::types::PollResults)> {
    let list = match updates {
        tl::enums::Updates::Updates(u) => Some(&u.updates),
        tl::enums::Updates::Combined(u) => Some(&u.updates),
        _ => None,
    }?;
    for update in list {
        if let tl::enums::Update::MessagePoll(m) = update {
            let poll = match m.poll.as_ref()? {
                tl::enums::Poll::Poll(p) => p.clone(),
            };
            let results = match &m.results {
                tl::enums::PollResults::Results(r) => r.as_ref().clone(),
            };
            return Some((poll, results));
        }
    }
    None
}

pub(crate) async fn poll_results_core(
    shares: &crate::client::ServeShares,
    params: PollResultsParams,
) -> TeleResult<serde_json::Value> {
    if params.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    let (chat, poll) = fetch_poll(shares, &params.chat, params.id).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    shares.rate_limiter.acquire().await;
    let updates: tl::enums::Updates = shares
        .client
        .invoke(&tl::functions::messages::GetPollResults {
            peer,
            msg_id: params.id,
            poll_hash: poll.raw.hash,
        })
        .await
        .map_err(tele_invocation)?;
    if let Some((fresh_poll, fresh_results)) = updates_poll_parts(&updates) {
        return Ok(serde_json::json!({
            "id": params.id,
            "poll": fresh_poll_row(params.id, &fresh_poll, &fresh_results)}));
    }
    let row = crate::serialize::poll_row(&poll);
    Ok(serde_json::json!({"id": params.id, "poll": row, "refreshed": true}))
}

pub(crate) async fn poll_votes_core(
    shares: &crate::client::ServeShares,
    params: PollVotesParams,
) -> TeleResult<serde_json::Value> {
    if params.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    if params.limit == 0 || params.limit > 100 {
        return Err(TeleError::Usage(
            "--limit must be between 1 and 100".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    let (chat, poll) = fetch_poll(shares, &params.chat, params.id).await?;
    let option_bytes = match params.option {
        None => None,
        Some(idx) => {
            let answers = crate::serialize::poll_answers(&poll);
            let (_, bytes) = answers.get(idx.wrapping_sub(1)).ok_or_else(|| {
                TeleError::Usage(format!(
                    "--option index {idx} is out of range (poll has {} option(s))",
                    answers.len()
                ))
            })?;
            Some(bytes.clone())
        }
    };
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    shares.rate_limiter.acquire().await;
    let list: tl::enums::messages::VotesList = shares
        .client
        .invoke(&tl::functions::messages::GetPollVotes {
            peer,
            id: params.id,
            option: option_bytes,
            offset: if params.offset.is_empty() {
                None
            } else {
                Some(params.offset.clone())
            },
            limit: params.limit as i32,
        })
        .await
        .map_err(tele_invocation)?;
    let tl::enums::messages::VotesList::List(list) = list;
    let votes: Vec<serde_json::Value> = list
        .votes
        .iter()
        .map(|v| match v {
            tl::enums::MessagePeerVote::Vote(v) => serde_json::json!({
                "peer_id": peer_id(&v.peer),
                "date": v.date}),
            tl::enums::MessagePeerVote::InputOption(v) => serde_json::json!({
                "peer_id": peer_id(&v.peer),
                "date": v.date}),
            tl::enums::MessagePeerVote::Multiple(v) => serde_json::json!({
                "peer_id": peer_id(&v.peer),
                "date": v.date}),
        })
        .collect();
    let mut out = serde_json::json!({
        "id": params.id,
        "count": list.count,
        "votes": votes});
    if let Some(next) = list.next_offset {
        out["next_offset"] = serde_json::json!(next);
    }
    Ok(out)
}

pub(crate) async fn poll_unread_core(
    shares: &crate::client::ServeShares,
    params: PollUnreadParams,
) -> TeleResult<serde_json::Value> {
    if params.limit == 0 || params.limit > 100 {
        return Err(TeleError::Usage(
            "--limit must be between 1 and 100".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    shares.rate_limiter.acquire().await;
    let res: tl::enums::messages::Messages = shares
        .client
        .invoke(&tl::functions::messages::GetUnreadPollVotes {
            peer,
            top_msg_id: params.top,
            offset_id: 0,
            add_offset: 0,
            limit: params.limit as i32,
            max_id: 0,
            min_id: 0,
        })
        .await
        .map_err(tele_invocation)?;
    let (count, messages) = match res {
        tl::enums::messages::Messages::Messages(m) => {
            (m.messages.len() as i32, m.messages)
        }
        tl::enums::messages::Messages::Slice(s) => (s.count, s.messages),
        tl::enums::messages::Messages::ChannelMessages(c) => (c.count, c.messages),
        tl::enums::messages::Messages::NotModified(_) => (0, Vec::new()),
    };
    let rows: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| match m {
            tl::enums::Message::Message(m) => serde_json::json!({
                "id": m.id,
                "date": m.date,
                "text": m.message,
                "peer_id": peer_id(&m.peer_id)}),
            tl::enums::Message::Service(m) => serde_json::json!({
                "id": m.id,
                "date": m.date,
                "service": true}),
            tl::enums::Message::Empty(m) => serde_json::json!({
                "id": m.id,
                "empty": true}),
        })
        .collect();
    Ok(serde_json::json!({"count": count, "messages": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close_args(chat: &str, id: i32) -> PollCloseArgs {
        PollCloseArgs {
            chat: crate::chat_target::ChatTarget::new_unchecked(chat.to_string()),
            id,
        }
    }

    fn results_args(chat: &str, id: i32) -> PollResultsArgs {
        PollResultsArgs {
            chat: crate::chat_target::ChatTarget::new_unchecked(chat.to_string()),
            id,
        }
    }

    fn votes_args(chat: &str, id: i32) -> PollVotesArgs {
        PollVotesArgs {
            chat: crate::chat_target::ChatTarget::new_unchecked(chat.to_string()),
            id,
            option: None,
            limit: 20,
            offset: String::new(),
        }
    }

    fn unread_args(chat: &str) -> PollUnreadArgs {
        PollUnreadArgs {
            chat: crate::chat_target::ChatTarget::new_unchecked(chat.to_string()),
            top: None,
            limit: 10,
        }
    }

    #[test]
    fn poll_close_rejects_bad_id_and_chat() {
        assert!(validate_poll_close(&close_args("me", 5)).is_ok());
        assert!(matches!(
            validate_poll_close(&close_args("me", 0)),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_poll_close(&close_args("me", -1)),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_poll_close(&close_args("  ", 5)),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_poll_close(&close_args("https://t.me/durov/42", 5)),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn poll_results_rejects_bad_id() {
        assert!(validate_poll_results(&results_args("me", 7)).is_ok());
        assert!(matches!(
            validate_poll_results(&results_args("me", 0)),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn poll_votes_validates_option_and_limit() {
        assert!(validate_poll_votes(&votes_args("me", 7)).is_ok());
        let mut bad_option = votes_args("me", 7);
        bad_option.option = Some(0);
        assert!(matches!(
            validate_poll_votes(&bad_option),
            Err(TeleError::Usage(_))
        ));
        let mut bad_limit = votes_args("me", 7);
        bad_limit.limit = 0;
        assert!(matches!(
            validate_poll_votes(&bad_limit),
            Err(TeleError::Usage(_))
        ));
        let mut big_limit = votes_args("me", 7);
        big_limit.limit = 101;
        assert!(matches!(
            validate_poll_votes(&big_limit),
            Err(TeleError::Usage(_))
        ));
        let mut bad_id = votes_args("me", 0);
        bad_id.option = Some(1);
        assert!(matches!(
            validate_poll_votes(&bad_id),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn poll_unread_validates_top_and_limit() {
        assert!(validate_poll_unread(&unread_args("me")).is_ok());
        let mut bad_top = unread_args("me");
        bad_top.top = Some(0);
        assert!(matches!(
            validate_poll_unread(&bad_top),
            Err(TeleError::Usage(_))
        ));
        let mut bad_limit = unread_args("me");
        bad_limit.limit = 101;
        assert!(matches!(
            validate_poll_unread(&bad_limit),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn poll_dry_runs_carry_would_text() {
        let v = poll_close_serve_dry_run(&close_args("@c", 9)).unwrap();
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["id"], serde_json::json!(9));
        assert!(v["would"].as_str().unwrap().contains("close poll"));

        let v = poll_results_serve_dry_run(&results_args("@c", 9)).unwrap();
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert!(v["would"].as_str().unwrap().contains("results"));

        let v = poll_votes_serve_dry_run(&votes_args("@c", 9)).unwrap();
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["limit"], serde_json::json!(20));

        let v = poll_unread_serve_dry_run(&unread_args("@c")).unwrap();
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["limit"], serde_json::json!(10));
    }

    #[test]
    fn poll_params_reject_unknown_fields() {
        assert!(serde_json::from_value::<PollCloseParams>(
            serde_json::json!({"chat": "me", "id": 1, "bogus": 1})
        )
        .is_err());
        assert!(serde_json::from_value::<PollVotesParams>(
            serde_json::json!({"chat": "me", "id": 1, "bogus": 1})
        )
        .is_err());
    }

    #[test]
    fn fresh_poll_row_shapes_options_with_voters() {
        let poll = tl::types::Poll {
            id: 3,
            closed: true,
            public_voters: false,
            multiple_choice: false,
            quiz: false,
            open_answers: false,
            revoting_disabled: false,
            shuffle_answers: false,
            hide_results_until_close: false,
            creator: false,
            subscribers_only: false,
            question: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                text: "Pick?".to_string(),
                entities: Vec::new(),
            }),
            answers: vec![
                tl::enums::PollAnswer::Answer(tl::types::PollAnswer {
                    text: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                        text: "A".to_string(),
                        entities: Vec::new(),
                    }),
                    option: b"a".to_vec(),
                    media: None,
                    added_by: None,
                    date: None,
                }),
                tl::enums::PollAnswer::Answer(tl::types::PollAnswer {
                    text: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                        text: "B".to_string(),
                        entities: Vec::new(),
                    }),
                    option: b"b".to_vec(),
                    media: None,
                    added_by: None,
                    date: None,
                }),
            ],
            close_period: None,
            close_date: None,
            countries_iso2: None,
            hash: 0,
        };
        let results = tl::types::PollResults {
            min: false,
            results: Some(vec![tl::enums::PollAnswerVoters::Voters(
                tl::types::PollAnswerVoters {
                    chosen: true,
                    correct: false,
                    option: b"a".to_vec(),
                    voters: Some(4),
                    recent_voters: None,
                },
            )]),
            total_voters: Some(9),
            recent_voters: None,
            solution: None,
            solution_entities: None,
            solution_media: None,
            has_unread_votes: false,
            can_view_stats: false,
        };
        let row = fresh_poll_row(11, &poll, &results);
        assert_eq!(row["id"], serde_json::json!(11));
        assert_eq!(row["question"], serde_json::json!("Pick?"));
        assert_eq!(row["closed"], serde_json::json!(true));
        assert_eq!(row["total_voters"], serde_json::json!(9));
        assert_eq!(row["options"][0]["voters"], serde_json::json!(4));
        assert!(row["options"][1].get("voters").is_none());
    }

    #[test]
    fn updates_poll_parts_extracts_message_poll_update() {
        let poll = tl::types::Poll {
            id: 3,
            closed: true,
            public_voters: false,
            multiple_choice: false,
            quiz: false,
            open_answers: false,
            revoting_disabled: false,
            shuffle_answers: false,
            hide_results_until_close: false,
            creator: false,
            subscribers_only: false,
            question: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                text: "Q".to_string(),
                entities: Vec::new(),
            }),
            answers: Vec::new(),
            close_period: None,
            close_date: None,
            countries_iso2: None,
            hash: 0,
        };
        let results = tl::types::PollResults {
            min: false,
            results: None,
            total_voters: Some(2),
            recent_voters: None,
            solution: None,
            solution_entities: None,
            solution_media: None,
            has_unread_votes: true,
            can_view_stats: false,
        };
        let updates = tl::enums::Updates::Updates(tl::types::Updates {
            updates: vec![tl::enums::Update::MessagePoll(tl::types::UpdateMessagePoll {
                poll_id: 3,
                poll: Some(tl::enums::Poll::Poll(poll)),
                results: tl::enums::PollResults::Results(Box::new(results)),
                peer: None,
                msg_id: None,
                top_msg_id: None,
            })],
            users: Vec::new(),
            chats: Vec::new(),
            date: 0,
            seq: 0,
        });
        let (p, r) = updates_poll_parts(&updates).expect("must extract");
        assert_eq!(p.id, 3);
        assert_eq!(r.total_voters, Some(2));
        assert!(r.has_unread_votes);

        let empty = tl::enums::Updates::TooLong;
        assert!(updates_poll_parts(&empty).is_none());
    }
}
