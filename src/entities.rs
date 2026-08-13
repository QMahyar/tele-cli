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
    let t = target.trim();
    if let Some(digits) = t
        .strip_prefix('+')
        .map(|p| p.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
    {
        if digits.is_empty() {
            return Err(rpc_error(400, "INVALID_PHONE"));
        }
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
        return match users.into_iter().next() {
            Some(user) => Ok(grammers_client::peer::Peer::User(
                grammers_client::peer::User::from_raw(client, user),
            )),
            None => Err(rpc_error(400, "USER_NOT_FOUND")),
        };
    }
    if let Ok(id) = t.parse::<i64>() {
        if id == 0 {
            return Err(rpc_error(400, "INVALID_PEER_ID"));
        }
        let raw = id.unsigned_abs() as i64;
        if let Some(pref) = cached_ref(session, id, raw).await {
            return client.resolve_peer(pref).await;
        }
        if let Some(pref) = checked_fallback_ref(id) {
            return client.resolve_peer(pref).await;
        }
        return Err(rpc_error(400, "INVALID_PEER_ID"));
    }
    let username = parse_username(t);
    if username == "me" {
        let user = client.get_me().await?;
        return Ok(grammers_client::peer::Peer::User(user));
    }
    match client.resolve_username(username).await? {
        Some(peer) => Ok(peer),
        None => Err(rpc_error(400, "USERNAME_NOT_FOUND")),
    }
}

async fn cached_ref<S: Session>(session: &S, id: i64, raw: i64) -> Option<PeerRef> {
    let ids: Vec<PeerId> = if id > 0 {
        [PeerId::user(raw), PeerId::channel(raw)]
            .into_iter()
            .flatten()
            .collect()
    } else {
        [PeerId::channel(raw), PeerId::chat(raw)]
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
    let raw = id.unsigned_abs() as i64;
    if id > 0 {
        PeerId::user(raw).map(|_| {
            tl::types::InputPeerUser {
                user_id: raw,
                access_hash: 0,
            }
            .into()
        })
    } else {
        PeerId::channel(raw).map(|_| {
            tl::types::InputPeerChannel {
                channel_id: raw,
                access_hash: 0,
            }
            .into()
        })
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
    use grammers_session::types::PeerId;

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
    fn checked_fallback_ref_accepts_valid_channel_id() {
        let pref = checked_fallback_ref(-3975233726).expect("valid channel id must resolve");
        assert_eq!(pref.id.bare_id_unchecked(), 3975233726);
    }

    #[test]
    fn checked_fallback_ref_rejects_out_of_range_channel() {
        assert!(checked_fallback_ref(-1001234567890).is_none());
    }

    #[test]
    fn checked_fallback_ref_rejects_i64_min() {
        assert!(checked_fallback_ref(i64::MIN).is_none());
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
    async fn cached_ref_misses_for_unknown_peer() {
        let session = MemorySession::default();
        assert!(cached_ref(&session, 12345, 12345).await.is_none());
    }
}
