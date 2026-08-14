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

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_client::tl;

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
}
