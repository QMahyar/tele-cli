use grammers_client::media::Media;
use grammers_client::peer::Peer;
use grammers_client::tl;

use crate::error::TeleError;

pub fn peer_name(peer: &Peer) -> String {
    peer.name()
        .map(|n| n.to_string())
        .unwrap_or_else(|| peer.id().to_string())
}

pub fn peer_kind(peer: &Peer) -> &'static str {
    match peer {
        Peer::User(_) => "user",
        Peer::Group(_) => "group",
        Peer::Channel(_) => "channel",
    }
}

pub fn message_to_json(
    msg: &grammers_client::message::Message,
) -> Result<serde_json::Value, TeleError> {
    let mut out = serde_json::Map::new();
    out.insert("id".into(), serde_json::json!(msg.id()));
    out.insert("date".into(), serde_json::json!(msg.date().to_rfc3339()));
    out.insert("out".into(), serde_json::json!(msg.outgoing()));
    out.insert(
        "peer".into(),
        match msg.peer() {
            Some(peer) => serde_json::json!(peer_key(peer)),
            None => serde_json::Value::Null,
        },
    );
    out.insert(
        "sender".into(),
        match msg.sender() {
            Some(sender) => serde_json::json!(peer_key(sender)),
            None => serde_json::Value::Null,
        },
    );
    out.insert("text".into(), serde_json::json!(msg.text()));
    if let Some(media) = msg.media() {
        out.insert("media".into(), serde_json::json!(media_name(&media)));
        out.insert("media_kind".into(), serde_json::json!(media_kind(&media)));
        let label = match media_label(&media) {
            Some(label) => serde_json::json!(label),
            None => serde_json::Value::Null,
        };
        out.insert("media_label".into(), label);
    }
    if let Some(grouped_id) = msg.grouped_id() {
        out.insert("grouped_id".into(), serde_json::json!(grouped_id));
    }
    if let Some(views) = msg.view_count() {
        out.insert("views".into(), serde_json::json!(views));
    }
    if let Some(forwards) = msg.forward_count() {
        out.insert("forwards".into(), serde_json::json!(forwards));
    }
    if let Some(edit_date) = msg.edit_date() {
        out.insert(
            "edit_date".into(),
            serde_json::json!(edit_date.to_rfc3339()),
        );
    }
    if let Some(reply_to) = msg.reply_to_message_id() {
        out.insert("reply_to".into(), serde_json::json!(reply_to));
    }
    if let Some(via_bot) = msg.via_bot_id() {
        out.insert("via_bot".into(), serde_json::json!(via_bot));
    }
    if let Some(markup) = msg.reply_markup() {
        out.insert("reply_markup".into(), reply_markup_to_json(&markup));
    }
    Ok(serde_json::Value::Object(out))
}

pub fn peer_key(peer: &Peer) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(id) = peer.id().bare_id() {
        out.insert("id".into(), serde_json::json!(id));
    }
    out.insert("kind".into(), serde_json::json!(peer_kind(peer)));
    out.insert("name".into(), serde_json::json!(peer_name(peer)));
    serde_json::Value::Object(out)
}

pub fn media_name(media: &Media) -> String {
    match media_parts(media) {
        (kind, Some(label)) => format!("{kind}:{label}"),
        (kind, None) => kind.to_string(),
    }
}

pub fn media_kind(media: &Media) -> &'static str {
    media_parts(media).0
}

pub fn media_label(media: &Media) -> Option<String> {
    media_parts(media).1
}

fn media_parts(media: &Media) -> (&'static str, Option<String>) {
    let name = |d: &grammers_client::media::Document| {
        d.raw
            .document
            .as_ref()
            .and_then(|doc| match doc {
                tl::enums::Document::Document(doc) => {
                    doc.attributes.iter().find_map(|attr| match attr {
                        tl::enums::DocumentAttribute::Filename(attr) => {
                            Some(attr.file_name.as_str())
                        }
                        _ => None,
                    })
                }
                tl::enums::Document::Empty(_) => None,
            })
            .map(str::to_string)
            .unwrap_or_else(|| "unnamed".into())
    };
    match media {
        Media::Photo(_) => ("photo", None),
        Media::Geo(_) => ("geo", None),
        Media::GeoLive(_) => ("geo_live", None),
        Media::Contact(c) => ("contact", Some(c.first_name().to_string())),
        Media::Document(d) => ("document", Some(name(d))),
        Media::Sticker(d) => ("sticker", Some(d.emoji().to_string())),
        Media::Poll(p) => (
            "poll",
            Some(match p.question() {
                grammers_client::tl::enums::TextWithEntities::Entities(t) => t.text.clone(),
            }),
        ),
        Media::Dice(d) => ("dice", Some(d.emoji().to_string())),
        Media::Venue(_) => ("venue", None),
        Media::WebPage(_) => ("webpage", None),
        _ => ("media", None),
    }
}

pub fn reply_markup_to_json(markup: &tl::enums::ReplyMarkup) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let rows = match markup {
        tl::enums::ReplyMarkup::ReplyInlineMarkup(m) => {
            out.insert("kind".into(), serde_json::json!("inline"));
            Some(&m.rows)
        }
        tl::enums::ReplyMarkup::ReplyKeyboardMarkup(m) => {
            out.insert("kind".into(), serde_json::json!("reply"));
            Some(&m.rows)
        }
        tl::enums::ReplyMarkup::ReplyKeyboardHide(_) => {
            out.insert("kind".into(), serde_json::json!("hide"));
            None
        }
        tl::enums::ReplyMarkup::ReplyKeyboardForceReply(_) => {
            out.insert("kind".into(), serde_json::json!("force_reply"));
            None
        }
    };
    let rows = rows
        .map(|rows| {
            serde_json::Value::Array(
                rows.iter()
                    .map(|row| match row {
                        tl::enums::KeyboardButtonRow::Row(row) => serde_json::Value::Array(
                            row.buttons.iter().map(keyboard_button_to_json).collect(),
                        ),
                    })
                    .collect(),
            )
        })
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    out.insert("rows".into(), rows);
    serde_json::Value::Object(out)
}

fn keyboard_button_to_json(button: &tl::enums::KeyboardButton) -> serde_json::Value {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let mut out = serde_json::Map::new();
    match button {
        tl::enums::KeyboardButton::Button(b) => {
            out.insert("text".into(), serde_json::json!(b.text));
        }
        tl::enums::KeyboardButton::Url(b) => {
            out.insert("text".into(), serde_json::json!(b.text));
            out.insert("url".into(), serde_json::json!(b.url));
        }
        tl::enums::KeyboardButton::Callback(b) => {
            out.insert("text".into(), serde_json::json!(b.text));
            out.insert(
                "callback_data".into(),
                serde_json::json!(STANDARD.encode(&b.data)),
            );
        }
        tl::enums::KeyboardButton::SwitchInline(b) => {
            out.insert("text".into(), serde_json::json!(b.text));
            let key = if b.same_peer {
                "switch_inline_query_current_chat"
            } else {
                "switch_inline_query"
            };
            out.insert(key.into(), serde_json::json!(b.query));
        }
        tl::enums::KeyboardButton::Buy(b) => {
            out.insert("text".into(), serde_json::json!(b.text));
            out.insert("buy".into(), serde_json::Value::Bool(true));
        }
        other => {
            out.insert(
                "text".into(),
                serde_json::json!(keyboard_button_text(other)),
            );
            out.insert(
                "raw_kind".into(),
                serde_json::json!(keyboard_button_raw_kind(other)),
            );
        }
    }
    serde_json::Value::Object(out)
}

fn keyboard_button_text(button: &tl::enums::KeyboardButton) -> &str {
    match button {
        tl::enums::KeyboardButton::Button(b) => b.text.as_str(),
        tl::enums::KeyboardButton::Url(b) => b.text.as_str(),
        tl::enums::KeyboardButton::Callback(b) => b.text.as_str(),
        tl::enums::KeyboardButton::RequestPhone(b) => b.text.as_str(),
        tl::enums::KeyboardButton::RequestGeoLocation(b) => b.text.as_str(),
        tl::enums::KeyboardButton::SwitchInline(b) => b.text.as_str(),
        tl::enums::KeyboardButton::Game(b) => b.text.as_str(),
        tl::enums::KeyboardButton::Buy(b) => b.text.as_str(),
        tl::enums::KeyboardButton::UrlAuth(b) => b.text.as_str(),
        tl::enums::KeyboardButton::InputKeyboardButtonUrlAuth(b) => b.text.as_str(),
        tl::enums::KeyboardButton::InputKeyboardButtonUserProfile(b) => b.text.as_str(),
        tl::enums::KeyboardButton::InputKeyboardButtonRequestPeer(b) => b.text.as_str(),
        tl::enums::KeyboardButton::RequestPoll(b) => b.text.as_str(),
        tl::enums::KeyboardButton::UserProfile(b) => b.text.as_str(),
        tl::enums::KeyboardButton::WebView(b) => b.text.as_str(),
        tl::enums::KeyboardButton::SimpleWebView(b) => b.text.as_str(),
        tl::enums::KeyboardButton::RequestPeer(b) => b.text.as_str(),
        tl::enums::KeyboardButton::Copy(b) => b.text.as_str(),
    }
}

fn keyboard_button_raw_kind(button: &tl::enums::KeyboardButton) -> &'static str {
    match button {
        tl::enums::KeyboardButton::Button(_) => "Button",
        tl::enums::KeyboardButton::Url(_) => "Url",
        tl::enums::KeyboardButton::Callback(_) => "Callback",
        tl::enums::KeyboardButton::RequestPhone(_) => "RequestPhone",
        tl::enums::KeyboardButton::RequestGeoLocation(_) => "RequestGeoLocation",
        tl::enums::KeyboardButton::SwitchInline(_) => "SwitchInline",
        tl::enums::KeyboardButton::Game(_) => "Game",
        tl::enums::KeyboardButton::Buy(_) => "Buy",
        tl::enums::KeyboardButton::UrlAuth(_) => "UrlAuth",
        tl::enums::KeyboardButton::InputKeyboardButtonUrlAuth(_) => "InputKeyboardButtonUrlAuth",
        tl::enums::KeyboardButton::InputKeyboardButtonUserProfile(_) => {
            "InputKeyboardButtonUserProfile"
        }
        tl::enums::KeyboardButton::InputKeyboardButtonRequestPeer(_) => {
            "InputKeyboardButtonRequestPeer"
        }
        tl::enums::KeyboardButton::RequestPoll(_) => "RequestPoll",
        tl::enums::KeyboardButton::UserProfile(_) => "UserProfile",
        tl::enums::KeyboardButton::WebView(_) => "WebView",
        tl::enums::KeyboardButton::SimpleWebView(_) => "SimpleWebView",
        tl::enums::KeyboardButton::RequestPeer(_) => "RequestPeer",
        tl::enums::KeyboardButton::Copy(_) => "Copy",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline_client() -> grammers_client::Client {
        let session = std::sync::Arc::new(grammers_session::storages::MemorySession::default());
        let pool = grammers_client::sender::SenderPool::new(session, 12345);
        grammers_client::Client::new(pool.handle)
    }

    fn make_user_peer(client: &grammers_client::Client, id: i64) -> Peer {
        Peer::User(grammers_client::peer::User::from_raw(
            client,
            tl::enums::User::Empty(tl::types::UserEmpty { id }),
        ))
    }

    fn make_group_peer(client: &grammers_client::Client, id: i64) -> Peer {
        Peer::Group(grammers_client::peer::Group::from_raw(
            client,
            tl::enums::Chat::Empty(tl::types::ChatEmpty { id }),
        ))
    }

    fn make_channel_peer(client: &grammers_client::Client, id: i64) -> Peer {
        Peer::Channel(grammers_client::peer::Channel::from_raw(
            client,
            tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
                broadcast: true,
                megagroup: false,
                monoforum: false,
                id,
                access_hash: 0,
                title: "test".to_string(),
                until_date: None,
            }),
        ))
    }

    #[test]
    fn peer_name_returns_string_for_user() {
        let client = offline_client();
        let peer = make_user_peer(&client, 42);
        let name = peer_name(&peer);
        assert!(!name.is_empty());
    }

    #[test]
    fn peer_kind_returns_user_for_user_peer() {
        let client = offline_client();
        let peer = make_user_peer(&client, 42);
        assert_eq!(peer_kind(&peer), "user");
    }

    #[test]
    fn peer_kind_returns_group_for_group_peer() {
        let client = offline_client();
        let peer = make_group_peer(&client, 42);
        assert_eq!(peer_kind(&peer), "group");
    }

    #[test]
    fn peer_kind_returns_channel_for_channel_peer() {
        let client = offline_client();
        let peer = make_channel_peer(&client, 42);
        assert_eq!(peer_kind(&peer), "channel");
    }

    #[test]
    fn peer_key_contains_id_kind_name() {
        let client = offline_client();
        let peer = make_user_peer(&client, 42);
        let key = peer_key(&peer);
        assert!(key.get("id").is_some(), "peer_key must have id");
        assert!(key.get("kind").is_some(), "peer_key must have kind");
        assert!(key.get("name").is_some(), "peer_key must have name");
        assert_eq!(key["kind"], "user");
    }

    fn make_message(
        client: &grammers_client::Client,
        id: i32,
        out: bool,
        text: &str,
        media: Option<tl::enums::MessageMedia>,
    ) -> grammers_client::message::Message {
        grammers_client::message::Message::from_raw_short_updates(
            client,
            tl::types::UpdateShortSentMessage {
                out,
                id,
                pts: 0,
                pts_count: 0,
                date: 1700000000,
                media,
                entities: None,
                ttl_period: None,
            },
            grammers_client::message::InputMessage::new().text(text),
            grammers_session::types::PeerId::user(42)
                .unwrap()
                .to_ambient_ref(),
        )
    }

    fn set_enrichment(
        msg: &mut grammers_client::message::Message,
        grouped_id: Option<i64>,
        views: Option<i32>,
        forwards: Option<i32>,
        edit_date: Option<i32>,
        reply_to_msg_id: Option<i32>,
        via_bot_id: Option<i64>,
    ) {
        if let tl::enums::Message::Message(m) = &mut msg.raw {
            m.grouped_id = grouped_id;
            m.views = views;
            m.forwards = forwards;
            m.edit_date = edit_date;
            m.via_bot_id = via_bot_id;
            m.reply_to = reply_to_msg_id.map(|id| {
                tl::enums::MessageReplyHeader::Header(tl::types::MessageReplyHeader {
                    reply_to_scheduled: false,
                    forum_topic: false,
                    quote: false,
                    reply_to_ephemeral: false,
                    reply_to_msg_id: Some(id),
                    reply_to_peer_id: None,
                    reply_from: None,
                    reply_media: None,
                    reply_to_top_id: None,
                    quote_text: None,
                    quote_entities: None,
                    quote_offset: None,
                    todo_item_id: None,
                    poll_option: None,
                })
            });
        }
    }

    fn set_reply_markup(
        msg: &mut grammers_client::message::Message,
        markup: Option<tl::enums::ReplyMarkup>,
    ) {
        if let tl::enums::Message::Message(m) = &mut msg.raw {
            m.reply_markup = markup;
        }
    }

    fn text_button(text: &str) -> tl::enums::KeyboardButton {
        tl::enums::KeyboardButton::Button(tl::types::KeyboardButton {
            style: None,
            text: text.into(),
        })
    }

    fn url_button(text: &str, url: &str) -> tl::enums::KeyboardButton {
        tl::enums::KeyboardButton::Url(tl::types::KeyboardButtonUrl {
            style: None,
            text: text.into(),
            url: url.into(),
        })
    }

    fn callback_button(text: &str, data: &[u8]) -> tl::enums::KeyboardButton {
        tl::enums::KeyboardButton::Callback(tl::types::KeyboardButtonCallback {
            requires_password: false,
            style: None,
            text: text.into(),
            data: data.to_vec(),
        })
    }

    fn switch_inline_button(same_peer: bool, query: &str) -> tl::enums::KeyboardButton {
        tl::enums::KeyboardButton::SwitchInline(tl::types::KeyboardButtonSwitchInline {
            same_peer,
            style: None,
            text: "switch".into(),
            query: query.into(),
            peer_types: None,
        })
    }

    fn buy_button() -> tl::enums::KeyboardButton {
        tl::enums::KeyboardButton::Buy(tl::types::KeyboardButtonBuy {
            style: None,
            text: "buy".into(),
        })
    }

    fn request_phone_button() -> tl::enums::KeyboardButton {
        tl::enums::KeyboardButton::RequestPhone(tl::types::KeyboardButtonRequestPhone {
            style: None,
            text: "phone".into(),
        })
    }

    fn inline_markup(rows: Vec<Vec<tl::enums::KeyboardButton>>) -> tl::enums::ReplyMarkup {
        tl::enums::ReplyMarkup::ReplyInlineMarkup(tl::types::ReplyInlineMarkup {
            rows: rows
                .into_iter()
                .map(|buttons| {
                    tl::enums::KeyboardButtonRow::Row(tl::types::KeyboardButtonRow { buttons })
                })
                .collect(),
        })
    }

    fn keyboard_markup(rows: Vec<Vec<tl::enums::KeyboardButton>>) -> tl::enums::ReplyMarkup {
        tl::enums::ReplyMarkup::ReplyKeyboardMarkup(tl::types::ReplyKeyboardMarkup {
            resize: false,
            single_use: false,
            selective: false,
            persistent: false,
            rows: rows
                .into_iter()
                .map(|buttons| {
                    tl::enums::KeyboardButtonRow::Row(tl::types::KeyboardButtonRow { buttons })
                })
                .collect(),
            placeholder: None,
        })
    }

    fn action_keys(button: &serde_json::Value) -> usize {
        [
            "callback_data",
            "url",
            "switch_inline_query",
            "switch_inline_query_current_chat",
            "buy",
        ]
        .iter()
        .filter(|key| button.get(*key).is_some())
        .count()
    }

    fn photo_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
            spoiler: false,
            live_photo: false,
            photo: Some(tl::enums::Photo::Photo(tl::types::Photo {
                has_stickers: false,
                id: 1,
                access_hash: 0,
                file_reference: Vec::new(),
                date: 1700000000,
                sizes: Vec::new(),
                video_sizes: None,
                dc_id: 2,
            })),
            ttl_seconds: None,
            video: None,
        })
    }

    fn document_media(attributes: Vec<tl::enums::DocumentAttribute>) -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Document(tl::types::MessageMediaDocument {
            nopremium: false,
            spoiler: false,
            video: false,
            round: false,
            voice: false,
            document: Some(tl::enums::Document::Document(tl::types::Document {
                id: 1,
                access_hash: 0,
                file_reference: Vec::new(),
                date: 1700000000,
                mime_type: "application/octet-stream".into(),
                size: 1024,
                thumbs: None,
                video_thumbs: None,
                dc_id: 2,
                attributes,
            })),
            alt_documents: None,
            video_cover: None,
            video_timestamp: None,
            ttl_seconds: None,
        })
    }

    fn document_media_without_object() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Document(tl::types::MessageMediaDocument {
            nopremium: false,
            spoiler: false,
            video: false,
            round: false,
            voice: false,
            document: None,
            alt_documents: None,
            video_cover: None,
            video_timestamp: None,
            ttl_seconds: None,
        })
    }

    fn contact_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Contact(tl::types::MessageMediaContact {
            phone_number: "000".into(),
            first_name: "Alice".into(),
            last_name: "Smith".into(),
            vcard: String::new(),
            user_id: 1,
        })
    }

    fn poll_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Poll(Box::new(tl::types::MessageMediaPoll {
            poll: tl::enums::Poll::Poll(tl::types::Poll {
                id: 1,
                closed: false,
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
                    text: "Best option?".into(),
                    entities: Vec::new(),
                }),
                answers: Vec::new(),
                close_period: None,
                close_date: None,
                countries_iso2: None,
                hash: 0,
            }),
            results: tl::enums::PollResults::Results(Box::new(tl::types::PollResults {
                min: false,
                has_unread_votes: false,
                can_view_stats: false,
                results: None,
                total_voters: None,
                recent_voters: None,
                solution: None,
                solution_entities: None,
                solution_media: None,
            })),
            attached_media: None,
        }))
    }

    fn dice_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Dice(tl::types::MessageMediaDice {
            value: 5,
            emoticon: "🎲".into(),
            game_outcome: None,
        })
    }

    fn venue_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Venue(tl::types::MessageMediaVenue {
            geo: tl::enums::GeoPoint::Point(tl::types::GeoPoint {
                long: 1.0,
                lat: 2.0,
                access_hash: 0,
                accuracy_radius: None,
            }),
            title: "Cafe".into(),
            address: "1 Main St".into(),
            provider: "foursquare".into(),
            venue_id: "id".into(),
            venue_type: "cafe".into(),
        })
    }

    fn webpage_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::WebPage(tl::types::MessageMediaWebPage {
            force_large_media: false,
            force_small_media: false,
            manual: false,
            safe: false,
            webpage: tl::enums::WebPage::Pending(tl::types::WebPagePending {
                id: 1,
                url: Some("https://example.com".into()),
                date: 1700000000,
            }),
        })
    }

    fn geo_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::Geo(tl::types::MessageMediaGeo {
            geo: tl::enums::GeoPoint::Point(tl::types::GeoPoint {
                long: 1.0,
                lat: 2.0,
                access_hash: 0,
                accuracy_radius: None,
            }),
        })
    }

    fn geo_live_media() -> tl::enums::MessageMedia {
        tl::enums::MessageMedia::GeoLive(tl::types::MessageMediaGeoLive {
            geo: tl::enums::GeoPoint::Point(tl::types::GeoPoint {
                long: 1.0,
                lat: 2.0,
                access_hash: 0,
                accuracy_radius: None,
            }),
            heading: None,
            period: 60,
            proximity_notification_radius: None,
        })
    }

    #[test]
    fn message_to_json_text_message_full_shape() {
        let client = offline_client();
        let msg = make_message(&client, 123, true, "hello", None);
        let value = message_to_json(&msg).unwrap();
        let obj = value.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["date", "id", "out", "peer", "sender", "text"]);
        assert_eq!(value["id"], 123);
        assert_eq!(value["date"], "2023-11-14T22:13:20+00:00");
        assert_eq!(value["out"], true);
        assert_eq!(value["text"], "hello");
        assert!(value["peer"].is_null());
        assert!(value["sender"].is_null());
        assert!(value.get("media").is_none());
    }

    #[test]
    fn message_to_json_incoming_message_has_out_false() {
        let client = offline_client();
        let msg = make_message(&client, 5, false, "hi", None);
        let value = message_to_json(&msg).unwrap();
        assert_eq!(value["out"], false);
        assert_eq!(value["id"], 5);
    }

    #[test]
    fn message_to_json_empty_text_is_empty_string() {
        let client = offline_client();
        let msg = make_message(&client, 1, false, "", None);
        let value = message_to_json(&msg).unwrap();
        assert_eq!(value["text"], "");
    }

    #[test]
    fn message_to_json_enriched_fields_present_when_set() {
        let client = offline_client();
        let mut msg = make_message(&client, 7, true, "hello", None);
        set_enrichment(
            &mut msg,
            Some(9001),
            Some(12),
            Some(3),
            Some(1700000100),
            Some(55),
            Some(424242),
        );
        let value = message_to_json(&msg).unwrap();
        for key in [
            "grouped_id",
            "views",
            "forwards",
            "edit_date",
            "reply_to",
            "via_bot",
        ] {
            assert!(value.get(key).is_some(), "missing enriched key {key}");
        }
        assert_eq!(value["grouped_id"], 9001);
        assert_eq!(value["views"], 12);
        assert_eq!(value["forwards"], 3);
        assert_eq!(value["edit_date"], "2023-11-14T22:15:00+00:00");
        assert_eq!(value["reply_to"], 55);
        assert_eq!(value["via_bot"], 424242);
    }

    #[test]
    fn message_to_json_omits_absent_enrichment_fields() {
        let client = offline_client();
        let msg = make_message(&client, 7, false, "hello", None);
        let value = message_to_json(&msg).unwrap();
        for key in [
            "grouped_id",
            "views",
            "forwards",
            "edit_date",
            "reply_to",
            "via_bot",
        ] {
            assert!(
                value.get(key).is_none(),
                "{key} must be omitted when absent"
            );
        }
    }

    #[test]
    fn message_to_json_reply_markup_absent_key_omitted() {
        let client = offline_client();
        let msg = make_message(&client, 9, false, "plain", None);
        let value = message_to_json(&msg).unwrap();
        assert!(
            value.get("reply_markup").is_none(),
            "reply_markup must be omitted when absent"
        );
    }

    #[test]
    fn message_to_json_inline_markup_kind_rows_and_button_actions() {
        let client = offline_client();
        let markup = inline_markup(vec![
            vec![
                callback_button("Hit", b"\x00\x01\xfe\xff"),
                url_button("Go", "https://example.com"),
            ],
            vec![text_button("Plain")],
        ]);
        let mut msg = make_message(&client, 10, false, "pick", None);
        set_reply_markup(&mut msg, Some(markup));
        let value = message_to_json(&msg).unwrap();
        let rm = &value["reply_markup"];
        assert_eq!(rm["kind"], "inline");
        let rows = rm["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].as_array().unwrap().len(), 2);
        assert_eq!(rows[0][0]["text"], "Hit");
        assert_eq!(rows[0][0]["callback_data"], "AAH+/w==");
        assert_eq!(action_keys(&rows[0][0]), 1);
        assert_eq!(rows[0][1]["text"], "Go");
        assert_eq!(rows[0][1]["url"], "https://example.com");
        assert_eq!(action_keys(&rows[0][1]), 1);
        assert_eq!(rows[1][0], serde_json::json!({"text": "Plain"}));
    }

    #[test]
    fn message_to_json_reply_keyboard_kind_is_reply() {
        let client = offline_client();
        let markup = keyboard_markup(vec![
            vec![text_button("Yes"), text_button("No")],
            vec![text_button("Cancel")],
        ]);
        let mut msg = make_message(&client, 11, false, "choose", None);
        set_reply_markup(&mut msg, Some(markup));
        let value = message_to_json(&msg).unwrap();
        let rm = &value["reply_markup"];
        assert_eq!(rm["kind"], "reply");
        assert_eq!(rm["rows"][0][0]["text"], "Yes");
        assert_eq!(rm["rows"][1].as_array().unwrap().len(), 1);
        assert_eq!(action_keys(&rm["rows"][0][0]), 0);
    }

    #[test]
    fn message_to_json_hide_markup_has_empty_rows() {
        let client = offline_client();
        let markup = tl::enums::ReplyMarkup::ReplyKeyboardHide(tl::types::ReplyKeyboardHide {
            selective: false,
        });
        let mut msg = make_message(&client, 12, false, "done", None);
        set_reply_markup(&mut msg, Some(markup));
        let value = message_to_json(&msg).unwrap();
        assert_eq!(value["reply_markup"]["kind"], "hide");
        assert_eq!(value["reply_markup"]["rows"], serde_json::json!([]));
    }

    #[test]
    fn message_to_json_force_reply_kind() {
        let client = offline_client();
        let markup =
            tl::enums::ReplyMarkup::ReplyKeyboardForceReply(tl::types::ReplyKeyboardForceReply {
                single_use: true,
                selective: false,
                placeholder: None,
            });
        let mut msg = make_message(&client, 13, false, "answer", None);
        set_reply_markup(&mut msg, Some(markup));
        let value = message_to_json(&msg).unwrap();
        assert_eq!(value["reply_markup"]["kind"], "force_reply");
        assert_eq!(value["reply_markup"]["rows"], serde_json::json!([]));
    }

    #[test]
    fn message_to_json_switch_inline_same_peer_selects_current_chat_key() {
        let client = offline_client();
        let global = inline_markup(vec![vec![switch_inline_button(false, "search q")]]);
        let local = inline_markup(vec![vec![switch_inline_button(true, "search here")]]);
        let mut msg = make_message(&client, 14, false, "sw", None);
        set_reply_markup(&mut msg, Some(global));
        let value = message_to_json(&msg).unwrap();
        assert_eq!(
            value["reply_markup"]["rows"][0][0]["switch_inline_query"],
            "search q"
        );
        assert_eq!(action_keys(&value["reply_markup"]["rows"][0][0]), 1);
        set_reply_markup(&mut msg, Some(local));
        let value = message_to_json(&msg).unwrap();
        assert_eq!(
            value["reply_markup"]["rows"][0][0]["switch_inline_query_current_chat"],
            "search here"
        );
        assert_eq!(action_keys(&value["reply_markup"]["rows"][0][0]), 1);
    }

    #[test]
    fn message_to_json_buy_button_emits_buy_flag() {
        let client = offline_client();
        let markup = inline_markup(vec![vec![buy_button()]]);
        let mut msg = make_message(&client, 15, false, "shop", None);
        set_reply_markup(&mut msg, Some(markup));
        let value = message_to_json(&msg).unwrap();
        let button = &value["reply_markup"]["rows"][0][0];
        assert_eq!(button["buy"], true);
        assert_eq!(button["text"], "buy");
        assert_eq!(action_keys(button), 1);
    }

    #[test]
    fn message_to_json_unmapped_button_variant_falls_back_to_raw_kind() {
        let client = offline_client();
        let markup = inline_markup(vec![vec![request_phone_button()]]);
        let mut msg = make_message(&client, 16, false, "contact", None);
        set_reply_markup(&mut msg, Some(markup));
        let value = message_to_json(&msg).unwrap();
        let button = &value["reply_markup"]["rows"][0][0];
        assert_eq!(button["raw_kind"], "RequestPhone");
        assert_eq!(button["text"], "phone");
        assert_eq!(action_keys(button), 0);
    }

    #[test]
    fn message_to_json_photo_media_key_is_photo() {
        let client = offline_client();
        let msg = make_message(&client, 1, false, "", Some(photo_media()));
        let value = message_to_json(&msg).unwrap();
        assert_eq!(value["media"], "photo");
    }

    #[test]
    fn message_to_json_document_media_key_carries_filename() {
        let client = offline_client();
        let media = document_media(vec![tl::enums::DocumentAttribute::Filename(
            tl::types::DocumentAttributeFilename {
                file_name: "report.pdf".into(),
            },
        )]);
        let msg = make_message(&client, 1, false, "", Some(media));
        let value = message_to_json(&msg).unwrap();
        assert_eq!(value["media"], "document:report.pdf");
    }

    #[test]
    fn message_to_json_sticker_media_key_carries_emoji() {
        let client = offline_client();
        let media = document_media(vec![tl::enums::DocumentAttribute::Sticker(
            tl::types::DocumentAttributeSticker {
                mask: false,
                alt: "😂".into(),
                stickerset: tl::enums::InputStickerSet::Empty,
                mask_coords: None,
            },
        )]);
        let msg = make_message(&client, 1, false, "", Some(media));
        let value = message_to_json(&msg).unwrap();
        assert_eq!(value["media"], "sticker:😂");
    }

    #[test]
    fn media_name_photo_is_photo() {
        assert_eq!(
            media_name(&Media::from_raw(photo_media()).unwrap()),
            "photo"
        );
    }

    #[test]
    fn media_name_geo_is_geo() {
        assert_eq!(media_name(&Media::from_raw(geo_media()).unwrap()), "geo");
    }

    #[test]
    fn media_name_geo_live_is_geo_live() {
        assert_eq!(
            media_name(&Media::from_raw(geo_live_media()).unwrap()),
            "geo_live"
        );
    }

    #[test]
    fn media_name_contact_carries_first_name() {
        assert_eq!(
            media_name(&Media::from_raw(contact_media()).unwrap()),
            "contact:Alice"
        );
    }

    #[test]
    fn media_name_document_carries_filename() {
        let media = document_media(vec![tl::enums::DocumentAttribute::Filename(
            tl::types::DocumentAttributeFilename {
                file_name: "report.pdf".into(),
            },
        )]);
        assert_eq!(
            media_name(&Media::from_raw(media).unwrap()),
            "document:report.pdf"
        );
    }

    #[test]
    fn media_name_document_without_filename_is_unnamed() {
        let media = document_media(Vec::new());
        assert_eq!(
            media_name(&Media::from_raw(media).unwrap()),
            "document:unnamed"
        );
    }

    #[test]
    fn media_name_video_document_carries_filename() {
        let media = document_media(vec![
            tl::enums::DocumentAttribute::Video(tl::types::DocumentAttributeVideo {
                round_message: false,
                supports_streaming: true,
                nosound: false,
                duration: 10.0,
                w: 1920,
                h: 1080,
                preload_prefix_size: None,
                video_start_ts: None,
                video_codec: None,
            }),
            tl::enums::DocumentAttribute::Filename(tl::types::DocumentAttributeFilename {
                file_name: "clip.mp4".into(),
            }),
        ]);
        assert_eq!(
            media_name(&Media::from_raw(media).unwrap()),
            "document:clip.mp4"
        );
    }

    #[test]
    fn media_name_voice_note_without_filename_is_unnamed() {
        let media = document_media(vec![tl::enums::DocumentAttribute::Audio(
            tl::types::DocumentAttributeAudio {
                voice: true,
                duration: 3,
                title: None,
                performer: None,
                waveform: None,
            },
        )]);
        assert_eq!(
            media_name(&Media::from_raw(media).unwrap()),
            "document:unnamed"
        );
    }

    #[test]
    fn media_name_sticker_carries_emoji() {
        let media = document_media(vec![tl::enums::DocumentAttribute::Sticker(
            tl::types::DocumentAttributeSticker {
                mask: false,
                alt: "🎉".into(),
                stickerset: tl::enums::InputStickerSet::Empty,
                mask_coords: None,
            },
        )]);
        assert_eq!(media_name(&Media::from_raw(media).unwrap()), "sticker:🎉");
    }

    #[test]
    fn media_name_poll_carries_question() {
        assert_eq!(
            media_name(&Media::from_raw(poll_media()).unwrap()),
            "poll:Best option?"
        );
    }

    #[test]
    fn media_name_dice_carries_emoticon() {
        assert_eq!(
            media_name(&Media::from_raw(dice_media()).unwrap()),
            "dice:🎲"
        );
    }

    #[test]
    fn media_name_venue_is_venue() {
        assert_eq!(
            media_name(&Media::from_raw(venue_media()).unwrap()),
            "venue"
        );
    }

    #[test]
    fn media_name_webpage_is_webpage() {
        assert_eq!(
            media_name(&Media::from_raw(webpage_media()).unwrap()),
            "webpage"
        );
    }

    #[test]
    fn media_name_document_without_document_object_does_not_panic() {
        assert_eq!(
            media_name(&Media::from_raw(document_media_without_object()).unwrap()),
            "document:unnamed"
        );
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn message_to_json_never_panics_on_arbitrary_text(text in "\\PC{0,500}") {
            let client = offline_client();
            let msg = make_message(&client, 1, false, &text, None);
            let value = message_to_json(&msg).unwrap();
            serde_json::to_string(&value).unwrap();
            assert_eq!(value["text"], text);
        }

        #[test]
        fn message_to_json_never_panics_on_enrichment_fields(
            text in "\\PC{0,200}",
            grouped_id in proptest::option::of(any::<i64>()),
            views in proptest::option::of(any::<i32>()),
            forwards in proptest::option::of(any::<i32>()),
            edit_date in proptest::option::of(any::<i32>()),
            reply_to in proptest::option::of(1..i32::MAX),
            via_bot in proptest::option::of(any::<i64>()),
        ) {
            let client = offline_client();
            let mut msg = make_message(&client, 1, false, &text, None);
            set_enrichment(
                &mut msg,
                grouped_id,
                views,
                forwards,
                edit_date,
                reply_to,
                via_bot,
            );
            let value = message_to_json(&msg).unwrap();
            serde_json::to_string(&value).unwrap();
            prop_assert_eq!(value.get("grouped_id").is_some(), grouped_id.is_some());
            prop_assert_eq!(value.get("views").is_some(), views.is_some());
            prop_assert_eq!(value.get("forwards").is_some(), forwards.is_some());
            prop_assert_eq!(value.get("edit_date").is_some(), edit_date.is_some());
            prop_assert_eq!(value.get("reply_to").is_some(), reply_to.is_some());
            prop_assert_eq!(value.get("via_bot").is_some(), via_bot.is_some());
        }

        #[test]
        fn peer_key_id_matches_bare_id_for_users(id in 1..1_000_000_000_i64) {
            let client = offline_client();
            let peer = make_user_peer(&client, id);
            let value = peer_key(&peer);
            assert_eq!(value["id"], id);
            assert_eq!(value["kind"], "user");
            assert!(value["name"].is_string());
        }

        #[test]
        fn message_to_json_never_panics_on_reply_markup(
            text in "\\PC{0,200}",
            query in "\\PC{0,200}",
            data in proptest::collection::vec(any::<u8>(), 0..64),
            same_peer in proptest::bool::ANY,
            variant in 0u8..5,
        ) {
            let client = offline_client();
            let markup = match variant {
                0 => inline_markup(vec![vec![
                    callback_button(&text, &data),
                    switch_inline_button(same_peer, &query),
                ]]),
                1 => inline_markup(vec![vec![url_button(&text, "https://example.com")], vec![buy_button(), text_button(&text)]]),
                2 => keyboard_markup(vec![vec![text_button(&text)]]),
                3 => tl::enums::ReplyMarkup::ReplyKeyboardHide(tl::types::ReplyKeyboardHide {
                    selective: false,
                }),
                _ => tl::enums::ReplyMarkup::ReplyKeyboardForceReply(
                    tl::types::ReplyKeyboardForceReply {
                        single_use: false,
                        selective: false,
                        placeholder: None,
                    },
                ),
            };
            let mut msg = make_message(&client, 1, false, &text, None);
            set_reply_markup(&mut msg, Some(markup));
            let value = message_to_json(&msg).unwrap();
            serde_json::to_string(&value).unwrap();
            prop_assert!(value.get("reply_markup").is_some());
        }
    }
}
