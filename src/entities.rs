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
            None => Err(InvocationError::Rpc(grammers_client::sender::RpcError {
                code: 400,
                name: "USER_NOT_FOUND".to_string(),
                value: None,
                caused_by: None,
            })),
        };
    }
    if let Ok(id) = t.parse::<i64>() {
        let pref: PeerRef = if id > 0 {
            tl::types::InputPeerUser {
                user_id: id,
                access_hash: 0,
            }
            .into()
        } else {
            tl::types::InputPeerChannel {
                channel_id: -id,
                access_hash: 0,
            }
            .into()
        };
        return client.resolve_peer(pref).await;
    }
    let username = t
        .strip_prefix('@')
        .or_else(|| t.strip_prefix("t.me/"))
        .unwrap_or(t);
    let username = username
        .split('/')
        .next()
        .unwrap_or(username)
        .split('?')
        .next()
        .unwrap_or(username);
    if username == "me" {
        let user = client.get_me().await?;
        return Ok(grammers_client::peer::Peer::User(user));
    }
    match client.resolve_username(username).await? {
        Some(peer) => Ok(peer),
        None => Err(InvocationError::Rpc(grammers_client::sender::RpcError {
            code: 400,
            name: "USERNAME_NOT_FOUND".to_string(),
            value: None,
            caused_by: None,
        })),
    }
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
