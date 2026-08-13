use grammers_client::tl;
use grammers_client::{Client, InvocationError};
use grammers_session::types::PeerRef;

pub async fn resolve_peer(
    client: &Client,
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
        let pref: PeerRef = if id > 0 {
            tl::types::InputPeerUser {
                user_id: id,
                access_hash: 0,
            }
            .into()
        } else {
            let channel_id = id
                .checked_neg()
                .ok_or_else(|| rpc_error(400, "INVALID_PEER_ID"))?;
            tl::types::InputPeerChannel {
                channel_id,
                access_hash: 0,
            }
            .into()
        };
        return client.resolve_peer(pref).await;
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
