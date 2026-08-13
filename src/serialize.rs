use grammers_client::media::Media;
use grammers_client::peer::Peer;

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
    }
    Ok(serde_json::Value::Object(out))
}

pub fn peer_key(peer: &Peer) -> serde_json::Value {
    serde_json::json!({
        "id": peer.id().bare_id().unwrap_or_default(),
        "kind": peer_kind(peer),
        "name": peer_name(peer),
    })
}

pub fn media_name(media: &Media) -> String {
    let name = |d: &grammers_client::media::Document| {
        d.name()
            .map(str::to_string)
            .unwrap_or_else(|| "unnamed".into())
    };
    match media {
        Media::Photo(_) => "photo".into(),
        Media::Geo(_) => "geo".into(),
        Media::GeoLive(_) => "geo_live".into(),
        Media::Contact(c) => format!("contact:{}", c.first_name()),
        Media::Document(d) => format!("document:{}", name(d)),
        Media::Sticker(d) => format!("sticker:{}", d.emoji()),
        Media::Poll(p) => format!(
            "poll:{}",
            match p.question() {
                grammers_client::tl::enums::TextWithEntities::Entities(t) => t.text.clone(),
            }
        ),
        Media::Dice(d) => format!("dice:{}", d.emoji()),
        Media::Venue(_) => "venue".into(),
        Media::WebPage(_) => "webpage".into(),
        _ => "media".into(),
    }
}
