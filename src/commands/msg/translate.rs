use clap::Args;
use grammers_client::tl;

use crate::entities;
use crate::error::{tele_invocation, TeleError, TeleResult};

#[derive(Args, Clone)]
pub struct TranslateArgs {
    #[arg(
        long,
        value_parser = clap::value_parser!(crate::chat_target::ChatTarget),
        help = "target chat holding the messages to translate (requires --ids)"
    )]
    pub(crate) chat: Option<crate::chat_target::ChatTarget>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "comma-separated message IDs to translate (requires --chat)"
    )]
    pub(crate) ids: Vec<i32>,
    #[arg(
        long,
        help = "free text to translate (repeatable; mutually exclusive with --chat/--ids)"
    )]
    pub(crate) text: Vec<String>,
    #[arg(long, help = "target language code, e.g. en, es, fa")]
    pub(crate) to_lang: String,
}

#[derive(Args, Clone)]
pub struct TranscribeArgs {
    #[arg(
        long,
        value_parser = clap::value_parser!(crate::chat_target::ChatTarget),
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: crate::chat_target::ChatTarget,
    #[arg(long, help = "voice/video-note message ID to transcribe")]
    pub(crate) id: i32,
}

#[derive(Args, Clone)]
pub struct TranscribeRateArgs {
    #[arg(
        long,
        value_parser = clap::value_parser!(crate::chat_target::ChatTarget),
        help = "target chat: @username, t.me link, numeric ID, +phone, or me"
    )]
    pub(crate) chat: crate::chat_target::ChatTarget,
    #[arg(long, help = "transcribed message ID")]
    pub(crate) id: i32,
    #[arg(long, help = "transcription id returned by transcribe")]
    pub(crate) transcription_id: i64,
    #[arg(long, help = "rate the transcription as good")]
    pub(crate) good: bool,
    #[arg(long, help = "rate the transcription as bad")]
    pub(crate) bad: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct TranslateParams {
    #[serde(default)]
    pub(crate) chat: Option<String>,
    #[serde(default)]
    pub(crate) ids: Vec<i32>,
    #[serde(default)]
    pub(crate) text: Vec<String>,
    #[serde(default)]
    pub(crate) to_lang: String,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct TranscribeParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct TranscribeRateParams {
    #[serde(default)]
    pub(crate) chat: String,
    pub(crate) id: i32,
    pub(crate) transcription_id: i64,
    #[serde(default)]
    pub(crate) good: bool,
    #[serde(default)]
    pub(crate) bad: bool,
    #[serde(default)]
    pub(crate) dry_run: bool,
}

impl From<&TranslateArgs> for TranslateParams {
    fn from(a: &TranslateArgs) -> Self {
        Self {
            chat: a.chat.as_ref().map(|c| c.as_str().to_string()),
            ids: a.ids.clone(),
            text: a.text.clone(),
            to_lang: a.to_lang.clone(),
            dry_run: false,
        }
    }
}

impl From<&TranslateParams> for TranslateArgs {
    fn from(p: &TranslateParams) -> Self {
        Self {
            chat: p
                .chat
                .clone()
                .map(crate::chat_target::ChatTarget::new_unchecked),
            ids: p.ids.clone(),
            text: p.text.clone(),
            to_lang: p.to_lang.clone(),
        }
    }
}

impl From<&TranscribeArgs> for TranscribeParams {
    fn from(a: &TranscribeArgs) -> Self {
        Self {
            chat: a.chat.as_str().to_string(),
            id: a.id,
            dry_run: false,
        }
    }
}

impl From<&TranscribeParams> for TranscribeArgs {
    fn from(p: &TranscribeParams) -> Self {
        Self {
            chat: crate::chat_target::ChatTarget::new_unchecked(p.chat.clone()),
            id: p.id,
        }
    }
}

impl From<&TranscribeRateArgs> for TranscribeRateParams {
    fn from(a: &TranscribeRateArgs) -> Self {
        Self {
            chat: a.chat.as_str().to_string(),
            id: a.id,
            transcription_id: a.transcription_id,
            good: a.good,
            bad: a.bad,
            dry_run: false,
        }
    }
}

impl From<&TranscribeRateParams> for TranscribeRateArgs {
    fn from(p: &TranscribeRateParams) -> Self {
        Self {
            chat: crate::chat_target::ChatTarget::new_unchecked(p.chat.clone()),
            id: p.id,
            transcription_id: p.transcription_id,
            good: p.good,
            bad: p.bad,
        }
    }
}

pub(crate) fn validate_translate(args: &TranslateArgs) -> TeleResult<()> {
    if args.to_lang.trim().is_empty() {
        return Err(TeleError::Usage("--to-lang must not be empty".to_string()));
    }
    let by_message = args.chat.is_some() || !args.ids.is_empty();
    let by_text = !args.text.is_empty();
    if by_message && by_text {
        return Err(TeleError::Usage(
            "--text is mutually exclusive with --chat/--ids".to_string(),
        ));
    }
    if !by_message && !by_text {
        return Err(TeleError::Usage(
            "translate requires --chat with --ids, or --text".to_string(),
        ));
    }
    if let Some(chat) = &args.chat {
        crate::chat_target::ChatTarget::parse_flag(chat.as_str(), "chat")?;
        if args.ids.is_empty() {
            return Err(TeleError::Usage(
                "--chat requires --ids (message IDs to translate)".to_string(),
            ));
        }
    }
    if !args.ids.is_empty() && args.chat.is_none() {
        return Err(TeleError::Usage("--ids requires --chat".to_string()));
    }
    if args.ids.iter().any(|&i| i <= 0) {
        return Err(TeleError::Usage(
            "--ids must be positive message ids".to_string(),
        ));
    }
    if args.text.iter().any(|t| t.trim().is_empty()) {
        return Err(TeleError::Usage(
            "--text values must not be empty".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_transcribe(args: &TranscribeArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_transcribe_rate(args: &TranscribeRateArgs) -> TeleResult<()> {
    crate::chat_target::ChatTarget::parse_flag(args.chat.as_str(), "chat")?;
    if args.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    if args.transcription_id <= 0 {
        return Err(TeleError::Usage(
            "--transcription-id must be a positive transcription id".to_string(),
        ));
    }
    if args.good == args.bad {
        return Err(TeleError::Usage(
            "use --good or --bad, not both or neither".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn translate_serve_dry_run(args: &TranslateArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat.as_ref().map(|c| c.as_str()),
        "ids": args.ids,
        "text": args.text,
        "to_lang": args.to_lang,
        "would": format!("translate to {}", args.to_lang)}))
}

pub(crate) fn transcribe_serve_dry_run(args: &TranscribeArgs) -> TeleResult<serde_json::Value> {
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat.as_str(),
        "id": args.id,
        "would": format!("transcribe message {} in chat {}", args.id, args.chat.as_str())}))
}

pub(crate) fn transcribe_rate_serve_dry_run(
    args: &TranscribeRateArgs,
) -> TeleResult<serde_json::Value> {
    let verdict = if args.good { "good" } else { "bad" };
    Ok(serde_json::json!({
        "dry_run": true,
        "chat": args.chat.as_str(),
        "id": args.id,
        "transcription_id": args.transcription_id,
        "rating": verdict,
        "would": format!("rate transcription {} of message {} as {verdict}", args.transcription_id, args.id)}))
}

fn twe(text: &str) -> tl::enums::TextWithEntities {
    tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
        text: text.to_string(),
        entities: Vec::new(),
    })
}

pub(crate) async fn translate_core(
    shares: &crate::client::ServeShares,
    params: TranslateParams,
) -> TeleResult<serde_json::Value> {
    if params.to_lang.trim().is_empty() {
        return Err(TeleError::Usage("--to-lang must not be empty".to_string()));
    }
    let peer = match &params.chat {
        Some(target) => {
            shares.rate_limiter.acquire().await;
            let chat =
                entities::resolve_peer(&shares.client, shares.session.as_ref(), target).await?;
            Some(entities::input_peer(&chat).await.map_err(tele_invocation)?)
        }
        None => None,
    };
    let id = if params.ids.is_empty() {
        None
    } else {
        if params.chat.is_none() {
            return Err(TeleError::Usage("--ids requires --chat".to_string()));
        }
        if params.ids.iter().any(|&i| i <= 0) {
            return Err(TeleError::Usage(
                "--ids must be positive message ids".to_string(),
            ));
        }
        Some(params.ids.clone())
    };
    let text = if params.text.is_empty() {
        None
    } else {
        Some(params.text.iter().map(|t| twe(t)).collect())
    };
    if id.is_none() && text.is_none() {
        return Err(TeleError::Usage(
            "translate requires --chat with --ids, or --text".to_string(),
        ));
    }
    if id.is_some() && text.is_some() {
        return Err(TeleError::Usage(
            "--text is mutually exclusive with --chat/--ids".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    let res: tl::enums::messages::TranslatedText = shares
        .client
        .invoke(&tl::functions::messages::TranslateText {
            peer,
            id,
            text,
            to_lang: params.to_lang.clone(),
            tone: None,
        })
        .await
        .map_err(tele_invocation)?;
    let tl::enums::messages::TranslatedText::TranslateResult(res) = res;
    let translations = res
        .result
        .iter()
        .map(|t| match t {
            tl::enums::TextWithEntities::Entities(t) => t.text.clone(),
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "to_lang": params.to_lang,
        "count": translations.len(),
        "translations": translations}))
}

pub(crate) async fn transcribe_core(
    shares: &crate::client::ServeShares,
    params: TranscribeParams,
) -> TeleResult<serde_json::Value> {
    if params.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    shares.rate_limiter.acquire().await;
    let res: tl::enums::messages::TranscribedAudio = shares
        .client
        .invoke(&tl::functions::messages::TranscribeAudio {
            peer,
            msg_id: params.id,
        })
        .await
        .map_err(tele_invocation)?;
    let tl::enums::messages::TranscribedAudio::Audio(audio) = res;
    Ok(serde_json::json!({
        "id": params.id,
        "pending": audio.pending,
        "transcription_id": audio.transcription_id,
        "text": audio.text,
        "trial_remains_num": audio.trial_remains_num,
        "trial_remains_until_date": audio.trial_remains_until_date}))
}

pub(crate) async fn transcribe_rate_core(
    shares: &crate::client::ServeShares,
    params: TranscribeRateParams,
) -> TeleResult<serde_json::Value> {
    if params.id <= 0 {
        return Err(TeleError::Usage(
            "--id must be a positive message ID".to_string(),
        ));
    }
    if params.good == params.bad {
        return Err(TeleError::Usage(
            "use --good or --bad, not both or neither".to_string(),
        ));
    }
    shares.rate_limiter.acquire().await;
    let chat =
        entities::resolve_peer(&shares.client, shares.session.as_ref(), &params.chat).await?;
    let peer = entities::input_peer(&chat).await.map_err(tele_invocation)?;
    shares.rate_limiter.acquire().await;
    let ok: bool = shares
        .client
        .invoke(&tl::functions::messages::RateTranscribedAudio {
            peer,
            msg_id: params.id,
            transcription_id: params.transcription_id,
            good: params.good,
        })
        .await
        .map_err(tele_invocation)?;
    Ok(serde_json::json!({
        "id": params.id,
        "transcription_id": params.transcription_id,
        "rated_good": params.good,
        "ok": ok}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate_text_args() -> TranslateArgs {
        TranslateArgs {
            chat: None,
            ids: Vec::new(),
            text: vec!["hello".to_string()],
            to_lang: "es".to_string(),
        }
    }

    #[test]
    fn validate_translate_accepts_text_or_chat_ids() {
        assert!(validate_translate(&translate_text_args()).is_ok());
        let by_message = TranslateArgs {
            chat: Some(crate::chat_target::ChatTarget::new_unchecked(
                "me".to_string(),
            )),
            ids: vec![3],
            text: Vec::new(),
            to_lang: "fa".to_string(),
        };
        assert!(validate_translate(&by_message).is_ok());
    }

    #[test]
    fn validate_translate_rejects_blanks_and_mixes() {
        let mut blank_lang = translate_text_args();
        blank_lang.to_lang = "  ".to_string();
        assert!(matches!(
            validate_translate(&blank_lang),
            Err(TeleError::Usage(_))
        ));
        let mut mixed = translate_text_args();
        mixed.chat = Some(crate::chat_target::ChatTarget::new_unchecked(
            "me".to_string(),
        ));
        mixed.ids = vec![3];
        assert!(matches!(
            validate_translate(&mixed),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_translate(&TranslateArgs {
                chat: None,
                ids: Vec::new(),
                text: Vec::new(),
                to_lang: "es".to_string(),
            }),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_translate(&TranslateArgs {
                chat: Some(crate::chat_target::ChatTarget::new_unchecked(
                    "me".to_string(),
                )),
                ids: Vec::new(),
                text: Vec::new(),
                to_lang: "es".to_string(),
            }),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            validate_translate(&TranslateArgs {
                chat: None,
                ids: vec![3],
                text: Vec::new(),
                to_lang: "es".to_string(),
            }),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn validate_transcribe_rate_needs_exactly_one_verdict() {
        let base = TranscribeRateArgs {
            chat: crate::chat_target::ChatTarget::new_unchecked("me".to_string()),
            id: 9,
            transcription_id: 123,
            good: true,
            bad: false,
        };
        assert!(validate_transcribe_rate(&base).is_ok());
        for (good, bad) in [(true, true), (false, false)] {
            let args = TranscribeRateArgs {
                good,
                bad,
                ..base.clone()
            };
            let err = validate_transcribe_rate(&args).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)));
        }
        let mut bad_id = base.clone();
        bad_id.transcription_id = 0;
        assert!(matches!(
            validate_transcribe_rate(&bad_id),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn translate_transcribe_dry_runs_carry_would() {
        let v = translate_serve_dry_run(&translate_text_args()).unwrap();
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["to_lang"], serde_json::json!("es"));
        let v = transcribe_serve_dry_run(&TranscribeArgs {
            chat: crate::chat_target::ChatTarget::new_unchecked("me".to_string()),
            id: 9,
        })
        .unwrap();
        assert!(v["would"]
            .as_str()
            .unwrap()
            .contains("transcribe message 9"));
        let v = transcribe_rate_serve_dry_run(&TranscribeRateArgs {
            chat: crate::chat_target::ChatTarget::new_unchecked("me".to_string()),
            id: 9,
            transcription_id: 123,
            good: false,
            bad: true,
        })
        .unwrap();
        assert_eq!(v["rating"], serde_json::json!("bad"));
    }

    #[test]
    fn translate_params_reject_unknown_fields() {
        assert!(serde_json::from_value::<TranslateParams>(
            serde_json::json!({"to_lang": "es", "bogus": 1})
        )
        .is_err());
        assert!(serde_json::from_value::<TranscribeRateParams>(
            serde_json::json!({"chat": "me", "id": 1, "transcription_id": 2, "bogus": 1})
        )
        .is_err());
    }
}
