use grammers_client::tl;
use grammers_client::{Client, InvocationError};
use grammers_session::storages::SqliteSession;
use grammers_session::types::{PeerId, PeerInfo, PeerRef};
use grammers_session::Session;

pub async fn resolve_peer(
    client: &Client,
    session: &SqliteSession,
    target: &str,
) -> Result<grammers_client::peer::Peer, InvocationError> {
    match classify_target(target) {
        Target::Phone(digits) => {
            if digits.is_empty() {
                return Err(rpc_error(400, "INVALID_PHONE"));
            }
            crate::output::log_line(
                "warn",
                "looking up phone number; privacy settings may hide the account",
            );
            let res = client
                .invoke(&tl::functions::contacts::ImportContacts {
                    contacts: vec![tl::enums::InputContact::InputPhoneContact(
                        tl::types::InputPhoneContact {
                            client_id: digits.parse::<i64>().unwrap_or(0),
                            phone: digits,
                            first_name: String::new(),
                            last_name: String::new(),
                            note: None,
                        },
                    )],
                })
                .await?;
            let users = match res {
                tl::enums::contacts::ImportedContacts::Contacts(res) => res.users,
            };
            if let Some(user) = users.first() {
                let _ = client
                    .invoke(&tl::functions::contacts::DeleteContacts {
                        id: vec![imported_user_to_input_user(user)],
                    })
                    .await;
            }
            match users.into_iter().next() {
                Some(user) => Ok(grammers_client::peer::Peer::User(
                    grammers_client::peer::User::from_raw(client, user),
                )),
                None => Err(rpc_error(400, &phone_not_found_message())),
            }
        }
        Target::Numeric(id) => {
            if id == 0 {
                return Err(rpc_error(400, "INVALID_PEER_ID"));
            }
            if let Some(pref) = cached_dialog_ref(session, id).await {
                return client.resolve_peer(pref).await;
            }
            let raw = if id < 0 { negative_id_raw(id) } else { id };
            if let Some(pref) = cached_ref(session, id, raw).await {
                return client.resolve_peer(pref).await;
            }
            if let Some(pref) = checked_fallback_ref(id) {
                if id > 0 {
                    return match client.resolve_peer(pref).await {
                        Ok(peer) => Ok(peer),
                        Err(grammers_client::InvocationError::Dropped) => {
                            let chat_pref = tl::enums::InputPeer::Chat(tl::types::InputPeerChat {
                                chat_id: raw,
                            });
                            client.resolve_peer(chat_pref).await
                        }
                        Err(e) => Err(e),
                    };
                }
                return client.resolve_peer(pref).await;
            }
            Err(rpc_error(400, "INVALID_PEER_ID"))
        }
        Target::Me => {
            let user = client.get_me().await?;
            Ok(grammers_client::peer::Peer::User(user))
        }
        Target::Link(username) | Target::Username(username) => {
            match client.resolve_username(&username).await? {
                Some(peer) => Ok(peer),
                None => Err(rpc_error(400, "USERNAME_NOT_FOUND")),
            }
        }
        Target::Invalid => Err(rpc_error(400, "INVALID_PEER_ID")),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Phone(String),
    Me,
    Link(String),
    Numeric(i64),
    Username(String),
    Invalid,
}

fn classify_target(raw: &str) -> Target {
    let t = raw.trim();
    if let Some(digits) = t
        .strip_prefix('+')
        .map(|p| p.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
    {
        return Target::Phone(digits);
    }
    if let Ok(id) = t.parse::<i64>() {
        return Target::Numeric(id);
    }
    let username = parse_username(t);
    if username == "me" {
        return Target::Me;
    }
    if is_link(t) {
        return Target::Link(username.to_string());
    }
    if username.is_empty() {
        return Target::Invalid;
    }
    Target::Username(username.to_string())
}

fn is_link(raw: &str) -> bool {
    let mut s = raw;
    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
        }
    }
    s.starts_with("t.me/") || s.starts_with("telegram.me/")
}

async fn cached_dialog_ref<S: Session>(session: &S, id: i64) -> Option<PeerRef> {
    let pid = PeerId::from_bot_api_dialog_id(id)?;
    session.peer_ref(pid).await.ok().flatten()
}

const CHANNEL_STYLE_BOUNDARY: i64 = -1000000000000;

fn negative_id_raw(id: i64) -> i64 {
    if id < CHANNEL_STYLE_BOUNDARY {
        (id.unsigned_abs() - 1000000000000) as i64
    } else {
        id.unsigned_abs() as i64
    }
}

fn is_channel_class(id: i64) -> bool {
    id < CHANNEL_STYLE_BOUNDARY
}

const MAX_PEER_RAW: i64 = 3000000000000;

fn in_peer_raw_range(raw: i64) -> bool {
    (1..=MAX_PEER_RAW).contains(&raw)
}

async fn cached_ref<S: Session>(session: &S, id: i64, raw: i64) -> Option<PeerRef> {
    if id < 0 && !in_peer_raw_range(raw) {
        return None;
    }
    let ids: Vec<PeerId> = if id > 0 {
        [PeerId::user(raw), PeerId::chat(raw), PeerId::channel(raw)]
            .into_iter()
            .flatten()
            .collect()
    } else if is_channel_class(id) {
        [PeerId::channel(raw), PeerId::chat(raw)]
            .into_iter()
            .flatten()
            .collect()
    } else {
        [PeerId::chat(raw), PeerId::channel(raw)]
            .into_iter()
            .flatten()
            .collect()
    };
    for pid in ids {
        if let Ok(Some(pref)) = session.peer_ref(pid).await {
            return Some(pref);
        }
    }
    None
}

fn checked_fallback_ref(id: i64) -> Option<PeerRef> {
    if id > 0 {
        return PeerId::user(id).map(|_| {
            tl::types::InputPeerUser {
                user_id: id,
                access_hash: 0,
            }
            .into()
        });
    }
    let raw = negative_id_raw(id);
    if !in_peer_raw_range(raw) {
        return None;
    }
    if is_channel_class(id) {
        PeerId::channel(raw).map(|_| {
            tl::types::InputPeerChannel {
                channel_id: raw,
                access_hash: 0,
            }
            .into()
        })
    } else {
        PeerId::chat(raw).map(|_| tl::types::InputPeerChat { chat_id: raw }.into())
    }
}

fn parse_username(raw: &str) -> &str {
    let mut s = raw;
    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
        }
    }
    for prefix in ["t.me/", "telegram.me/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
        }
    }
    for sep in ['/', '?', '#'] {
        if let Some(head) = s.split(sep).next() {
            s = head;
        }
    }
    s.strip_prefix('@').unwrap_or(s)
}

fn rpc_error(code: i32, name: &str) -> InvocationError {
    InvocationError::Rpc(grammers_client::sender::RpcError {
        code,
        name: name.to_string(),
        value: None,
        caused_by: None,
    })
}

fn phone_not_found_message() -> String {
    "phone number not found: it may be unregistered, or its owner hides it from phone-number search (ask the person to message you first)".to_string()
}

fn imported_user_to_input_user(user: &tl::enums::User) -> tl::enums::InputUser {
    match user {
        tl::enums::User::Empty(u) => tl::enums::InputUser::User(tl::types::InputUser {
            user_id: u.id,
            access_hash: 0,
        }),
        tl::enums::User::User(u) => tl::enums::InputUser::User(tl::types::InputUser {
            user_id: u.id,
            access_hash: u.access_hash.unwrap_or(0),
        }),
    }
}

pub fn is_channel(peer: &grammers_client::peer::Peer) -> bool {
    match peer {
        grammers_client::peer::Peer::Channel(_) => true,
        grammers_client::peer::Peer::Group(group) => matches!(
            &group.raw,
            tl::enums::Chat::Channel(_) | tl::enums::Chat::ChannelForbidden(_)
        ),
        grammers_client::peer::Peer::User(_) => false,
    }
}

pub async fn cache_chat<S: Session>(session: &S, chat: &tl::enums::Chat) -> Result<(), S::Error> {
    session.cache_peer(&PeerInfo::from(chat)).await
}

pub async fn peer_ref(peer: &grammers_client::peer::Peer) -> Result<PeerRef, InvocationError> {
    match peer.to_ref().await? {
        Some(pref) => Ok(pref),
        None => Err(InvocationError::Rpc(grammers_client::sender::RpcError {
            code: 400,
            name: "PEER_NOT_CACHED".to_string(),
            value: None,
            caused_by: None,
        })),
    }
}

pub async fn input_peer(
    peer: &grammers_client::peer::Peer,
) -> Result<tl::enums::InputPeer, InvocationError> {
    Ok(peer_ref(peer).await?.into())
}

pub async fn input_channel(
    peer: &grammers_client::peer::Peer,
) -> Result<tl::enums::InputChannel, InvocationError> {
    match input_peer(peer).await? {
        tl::enums::InputPeer::Channel(c) => {
            Ok(tl::enums::InputChannel::Channel(tl::types::InputChannel {
                channel_id: c.channel_id,
                access_hash: c.access_hash,
            }))
        }
        _ => Err(InvocationError::Rpc(grammers_client::sender::RpcError {
            code: 400,
            name: "CHAT_NOT_CHANNEL".to_string(),
            value: None,
            caused_by: None,
        })),
    }
}

pub async fn input_user(
    peer: &grammers_client::peer::Peer,
) -> Result<tl::enums::InputUser, InvocationError> {
    match input_peer(peer).await? {
        tl::enums::InputPeer::User(u) => Ok(tl::enums::InputUser::User(tl::types::InputUser {
            user_id: u.user_id,
            access_hash: u.access_hash,
        })),
        _ => Err(InvocationError::Rpc(grammers_client::sender::RpcError {
            code: 400,
            name: "PEER_NOT_USER".to_string(),
            value: None,
            caused_by: None,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_session::storages::MemorySession;
    use grammers_session::types::{PeerAuth, PeerId, PeerInfo, PeerKind};

    #[tokio::test]
    async fn cache_chat_stores_channel_access_hash() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 123456,
            access_hash: 987654321,
            title: "t".to_string(),
            until_date: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let peer = session
            .peer_ref(PeerId::channel_unchecked(123456))
            .await
            .unwrap()
            .expect("created chat must be cached");
        assert_eq!(peer.auth.hash(), 987654321);
    }

    #[tokio::test]
    async fn cache_chat_stores_basic_group() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::Chat(tl::types::Chat {
            creator: true,
            left: false,
            deactivated: false,
            call_active: false,
            call_not_empty: false,
            noforwards: false,
            id: 123,
            title: "g".to_string(),
            photo: tl::enums::ChatPhoto::Empty,
            participants_count: 1,
            date: 0,
            version: 1,
            migrated_to: None,
            admin_rights: None,
            default_banned_rights: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        assert!(session
            .peer_ref(PeerId::chat_unchecked(123))
            .await
            .unwrap()
            .is_some());
    }

    #[test]
    fn checked_fallback_ref_accepts_valid_user_id() {
        let pref = checked_fallback_ref(8997636887).expect("valid user id must resolve");
        assert_eq!(pref.id.bare_id_unchecked(), 8997636887);
    }

    #[test]
    fn checked_fallback_ref_accepts_plain_negative_id_as_basic_group() {
        let pref = checked_fallback_ref(-3975233726).expect("plain negative id must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Chat);
        assert_eq!(pref.id.bare_id_unchecked(), 3975233726);
    }

    #[test]
    fn checked_fallback_ref_accepts_bot_api_channel_id() {
        let pref = checked_fallback_ref(-1001234567890).expect("bot-api channel id must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Channel);
        assert_eq!(pref.id.bare_id_unchecked(), 1234567890);
    }

    #[test]
    fn negative_id_raw_channel_style_subtracts_base() {
        assert_eq!(negative_id_raw(-1001234567890), 1234567890);
        assert_eq!(negative_id_raw(-1000000000001), 1);
    }

    #[test]
    fn negative_id_raw_chat_style_is_abs() {
        assert_eq!(negative_id_raw(-1234567890), 1234567890);
        assert_eq!(negative_id_raw(-2), 2);
        assert_eq!(negative_id_raw(-1), 1);
    }

    #[test]
    fn negative_id_raw_empty_chat_sentinel_is_out_of_range() {
        assert_eq!(negative_id_raw(-1000000000000), 1000000000000);
        assert!(checked_fallback_ref(-1000000000000).is_none());
    }

    #[tokio::test]
    async fn cached_dialog_ref_resolves_cached_bot_api_channel() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 1234567890,
            access_hash: 8913517700375938783,
            title: "t".to_string(),
            until_date: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let pref = cached_dialog_ref(&session, -1001234567890)
            .await
            .expect("cached bot-api channel id must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Channel);
        assert_eq!(pref.id.bare_id_unchecked(), 1234567890);
        assert_eq!(pref.auth.hash(), 8913517700375938783);
    }

    #[test]
    fn checked_fallback_ref_rejects_i64_min() {
        assert!(checked_fallback_ref(i64::MIN).is_none());
    }

    #[test]
    fn checked_fallback_ref_accepts_channel_range_top() {
        let pref = checked_fallback_ref(-1997852516352).expect("channel range top must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Channel);
        assert_eq!(pref.id.bare_id_unchecked(), 997852516352);
    }

    #[test]
    fn checked_fallback_ref_accepts_monoforum_low_end() {
        let pref = checked_fallback_ref(-2002147483649).expect("monoforum low end must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Channel);
        assert_eq!(pref.id.bare_id_unchecked(), 1002147483649);
    }

    #[test]
    fn checked_fallback_ref_accepts_user_range_top() {
        let pref = checked_fallback_ref(0xffffffffff).expect("user range top must resolve");
        assert_eq!(pref.id.kind(), PeerKind::User);
        assert_eq!(pref.id.bare_id_unchecked(), 0xffffffffff);
    }

    #[test]
    fn checked_fallback_ref_rejects_user_id_over_range() {
        assert!(checked_fallback_ref(0x10000000000).is_none());
    }

    #[test]
    fn checked_fallback_ref_accepts_chat_range_top() {
        let pref = checked_fallback_ref(-999999999999).expect("chat range top must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Chat);
        assert_eq!(pref.id.bare_id_unchecked(), 999999999999);
    }

    #[test]
    fn checked_fallback_ref_rejects_empty_chat_sentinel() {
        assert!(checked_fallback_ref(-1000000000000).is_none());
    }

    #[tokio::test]
    async fn cached_ref_uses_stored_channel_hash() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 3975233726,
            access_hash: 8913517700375938783,
            title: "t".to_string(),
            until_date: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let pref = cached_ref(&session, -3975233726, 3975233726)
            .await
            .expect("cached channel must resolve");
        assert_eq!(pref.auth.hash(), 8913517700375938783);
    }

    #[tokio::test]
    async fn cached_ref_resolves_cached_basic_group() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::Chat(tl::types::Chat {
            creator: true,
            left: false,
            deactivated: false,
            call_active: false,
            call_not_empty: false,
            noforwards: false,
            id: 123,
            title: "g".to_string(),
            photo: tl::enums::ChatPhoto::Empty,
            participants_count: 1,
            date: 0,
            version: 1,
            migrated_to: None,
            admin_rights: None,
            default_banned_rights: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let pref = cached_ref(&session, -123, 123)
            .await
            .expect("cached basic group must resolve");
        assert_eq!(pref.id.bare_id_unchecked(), 123);
    }

    #[tokio::test]
    async fn cached_ref_resolves_positive_basic_group_chat_kind() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::Chat(tl::types::Chat {
            creator: true,
            left: false,
            deactivated: false,
            call_active: false,
            call_not_empty: false,
            noforwards: false,
            id: 123,
            title: "g".to_string(),
            photo: tl::enums::ChatPhoto::Empty,
            participants_count: 1,
            date: 0,
            version: 1,
            migrated_to: None,
            admin_rights: None,
            default_banned_rights: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let pref = cached_ref(&session, 123, 123)
            .await
            .expect("cached positive-id basic group must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Chat);
        assert_eq!(pref.id.bare_id_unchecked(), 123);
    }

    #[tokio::test]
    async fn cached_ref_misses_for_unknown_peer() {
        let session = MemorySession::default();
        assert!(cached_ref(&session, 12345, 12345).await.is_none());
    }

    #[tokio::test]
    async fn cached_ref_prefers_chat_over_channel_for_bare_negative_id() {
        let session = MemorySession::default();
        let channel = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 123456,
            access_hash: 111,
            title: "c".to_string(),
            until_date: None,
        });
        cache_chat(&session, &channel).await.unwrap();
        let chat = tl::enums::Chat::Chat(tl::types::Chat {
            creator: true,
            left: false,
            deactivated: false,
            call_active: false,
            call_not_empty: false,
            noforwards: false,
            id: 123456,
            title: "g".to_string(),
            photo: tl::enums::ChatPhoto::Empty,
            participants_count: 1,
            date: 0,
            version: 1,
            migrated_to: None,
            admin_rights: None,
            default_banned_rights: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let pref = cached_ref(&session, -123456, 123456)
            .await
            .expect("cached peer must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Chat);
        assert_eq!(pref.id.bare_id_unchecked(), 123456);
    }

    #[tokio::test]
    async fn cached_ref_prefers_channel_over_chat_for_channel_style_id() {
        let session = MemorySession::default();
        let channel = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 123456,
            access_hash: 111,
            title: "c".to_string(),
            until_date: None,
        });
        cache_chat(&session, &channel).await.unwrap();
        let chat = tl::enums::Chat::Chat(tl::types::Chat {
            creator: true,
            left: false,
            deactivated: false,
            call_active: false,
            call_not_empty: false,
            noforwards: false,
            id: 123456,
            title: "g".to_string(),
            photo: tl::enums::ChatPhoto::Empty,
            participants_count: 1,
            date: 0,
            version: 1,
            migrated_to: None,
            admin_rights: None,
            default_banned_rights: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let pref = cached_ref(&session, -1000000000123456, 123456)
            .await
            .expect("cached channel-style id must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Channel);
        assert_eq!(pref.auth.hash(), 111);
    }

    #[tokio::test]
    async fn cached_ref_prefers_user_over_channel_for_positive_id() {
        let session = MemorySession::default();
        session
            .cache_peer(&PeerInfo::User {
                id: 4242,
                auth: Some(PeerAuth::from_hash(222)),
                bot: Some(false),
                is_self: Some(false),
            })
            .await
            .unwrap();
        let channel = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 4242,
            access_hash: 333,
            title: "c".to_string(),
            until_date: None,
        });
        cache_chat(&session, &channel).await.unwrap();
        let pref = cached_ref(&session, 4242, 4242)
            .await
            .expect("cached user must resolve");
        assert_eq!(pref.id.kind(), PeerKind::User);
        assert_eq!(pref.auth.hash(), 222);
    }

    #[tokio::test]
    async fn cached_dialog_ref_resolves_cached_plain_user_id() {
        let session = MemorySession::default();
        session
            .cache_peer(&PeerInfo::User {
                id: 4242,
                auth: Some(PeerAuth::from_hash(222)),
                bot: Some(false),
                is_self: Some(false),
            })
            .await
            .unwrap();
        let pref = cached_dialog_ref(&session, 4242)
            .await
            .expect("cached plain id must resolve");
        assert_eq!(pref.id.kind(), PeerKind::User);
        assert_eq!(pref.id.bare_id_unchecked(), 4242);
        assert_eq!(pref.auth.hash(), 222);
    }

    #[tokio::test]
    async fn cached_dialog_ref_resolves_cached_basic_group() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::Chat(tl::types::Chat {
            creator: true,
            left: false,
            deactivated: false,
            call_active: false,
            call_not_empty: false,
            noforwards: false,
            id: 123,
            title: "g".to_string(),
            photo: tl::enums::ChatPhoto::Empty,
            participants_count: 1,
            date: 0,
            version: 1,
            migrated_to: None,
            admin_rights: None,
            default_banned_rights: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        let pref = cached_dialog_ref(&session, -123)
            .await
            .expect("cached basic group must resolve");
        assert_eq!(pref.id.kind(), PeerKind::Chat);
        assert_eq!(pref.id.bare_id_unchecked(), 123);
    }

    #[tokio::test]
    async fn cached_dialog_ref_misses_for_unknown_peer() {
        let session = MemorySession::default();
        assert!(cached_dialog_ref(&session, -1001234567890).await.is_none());
        assert!(cached_dialog_ref(&session, 12345).await.is_none());
    }

    #[tokio::test]
    async fn cached_dialog_ref_rejects_out_of_range_dialog_ids() {
        let session = MemorySession::default();
        assert!(cached_dialog_ref(&session, 0).await.is_none());
        assert!(cached_dialog_ref(&session, i64::MIN).await.is_none());
        assert!(cached_dialog_ref(&session, 0x10000000000).await.is_none());
    }

    #[tokio::test]
    async fn cached_dialog_ref_treats_negative_bare_id_as_chat_not_channel() {
        let session = MemorySession::default();
        let chat = tl::enums::Chat::ChannelForbidden(tl::types::ChannelForbidden {
            broadcast: true,
            megagroup: false,
            monoforum: false,
            id: 123456,
            access_hash: 111,
            title: "c".to_string(),
            until_date: None,
        });
        cache_chat(&session, &chat).await.unwrap();
        assert!(cached_dialog_ref(&session, -123456).await.is_none());
        let pref = cached_ref(&session, -123456, 123456)
            .await
            .expect("cached channel must still resolve via bare probe");
        assert_eq!(pref.auth.hash(), 111);
    }

    #[test]
    fn phone_not_found_message_explains_privacy() {
        let msg = phone_not_found_message();
        assert!(msg.contains("hides it from phone-number search"));
        assert!(msg.contains("message you first"));
    }

    #[test]
    fn imported_user_to_input_user_keeps_id_and_hash() {
        let user = tl::enums::User::User(tl::types::User {
            is_self: false,
            contact: false,
            mutual_contact: false,
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
            first_name: None,
            last_name: None,
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
        });
        assert_eq!(
            imported_user_to_input_user(&user),
            tl::enums::InputUser::User(tl::types::InputUser {
                user_id: 4242,
                access_hash: 777,
            })
        );
    }

    #[test]
    fn imported_user_to_input_user_maps_missing_hash_to_zero() {
        let user = tl::enums::User::User(tl::types::User {
            is_self: false,
            contact: false,
            mutual_contact: false,
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
            access_hash: None,
            first_name: None,
            last_name: None,
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
        });
        assert_eq!(
            imported_user_to_input_user(&user),
            tl::enums::InputUser::User(tl::types::InputUser {
                user_id: 4242,
                access_hash: 0,
            })
        );
    }

    #[test]
    fn imported_user_to_input_user_maps_empty_user_to_zero_hash() {
        assert_eq!(
            imported_user_to_input_user(&tl::enums::User::Empty(tl::types::UserEmpty { id: 5 })),
            tl::enums::InputUser::User(tl::types::InputUser {
                user_id: 5,
                access_hash: 0,
            })
        );
    }

    #[test]
    fn classify_phone_precedes_numeric_parse() {
        assert_eq!(
            classify_target("+989121234567"),
            Target::Phone("989121234567".to_string())
        );
    }

    #[test]
    fn classify_phone_keeps_only_ascii_digits() {
        assert_eq!(
            classify_target("+98 912 123 4567"),
            Target::Phone("989121234567".to_string())
        );
    }

    #[test]
    fn classify_bare_plus_is_phone_with_no_digits() {
        assert_eq!(classify_target("+"), Target::Phone(String::new()));
    }

    #[test]
    fn classify_me_is_self() {
        assert_eq!(classify_target("me"), Target::Me);
    }

    #[test]
    fn classify_at_me_is_self() {
        assert_eq!(classify_target("@me"), Target::Me);
    }

    #[test]
    fn classify_tme_me_link_is_self() {
        assert_eq!(classify_target("t.me/me"), Target::Me);
    }

    #[test]
    fn classify_https_tme_me_link_is_self() {
        assert_eq!(classify_target("https://t.me/me"), Target::Me);
    }

    #[test]
    fn classify_tme_link_is_link() {
        assert_eq!(
            classify_target("t.me/durov"),
            Target::Link("durov".to_string())
        );
    }

    #[test]
    fn classify_https_tme_link_with_path_is_link() {
        assert_eq!(
            classify_target("https://t.me/durov/42?x=1"),
            Target::Link("durov".to_string())
        );
    }

    #[test]
    fn classify_telegram_me_link_is_link() {
        assert_eq!(
            classify_target("telegram.me/durov"),
            Target::Link("durov".to_string())
        );
    }

    #[test]
    fn classify_http_telegram_me_link_is_link() {
        assert_eq!(
            classify_target("http://telegram.me/durov"),
            Target::Link("durov".to_string())
        );
    }

    #[test]
    fn classify_zero_id_is_numeric_zero() {
        assert_eq!(classify_target("0"), Target::Numeric(0));
    }

    #[test]
    fn classify_positive_id_is_numeric() {
        assert_eq!(classify_target("8997636887"), Target::Numeric(8997636887));
    }

    #[test]
    fn classify_i64_max_is_numeric() {
        assert_eq!(
            classify_target("9223372036854775807"),
            Target::Numeric(i64::MAX)
        );
    }

    #[test]
    fn classify_i64_min_is_numeric() {
        assert_eq!(
            classify_target("-9223372036854775808"),
            Target::Numeric(i64::MIN)
        );
    }

    #[test]
    fn classify_bot_api_channel_id_is_numeric() {
        assert_eq!(
            classify_target("-1001234567890"),
            Target::Numeric(-1001234567890)
        );
    }

    #[test]
    fn classify_bare_negative_group_id_is_numeric() {
        assert_eq!(classify_target("-123"), Target::Numeric(-123));
    }

    #[test]
    fn classify_at_username_is_username() {
        assert_eq!(
            classify_target("@durov"),
            Target::Username("durov".to_string())
        );
    }

    #[test]
    fn classify_bare_username_is_username() {
        assert_eq!(
            classify_target("durov"),
            Target::Username("durov".to_string())
        );
    }

    #[test]
    fn classify_literal_id_zero_text_is_username() {
        assert_eq!(
            classify_target("id=0"),
            Target::Username("id=0".to_string())
        );
    }

    #[test]
    fn classify_empty_string_is_invalid() {
        assert_eq!(classify_target(""), Target::Invalid);
    }

    #[test]
    fn classify_whitespace_only_is_invalid() {
        assert_eq!(classify_target(" \t "), Target::Invalid);
    }
}
