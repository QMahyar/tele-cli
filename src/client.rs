use std::sync::Arc;
use std::time::Duration;

use crate::error::{TeleError, TeleResult};
use crate::rate_limiter::RateLimiter;
use grammers_client::client::{AutoSleep, ClientConfiguration, UpdatesConfiguration};
use grammers_client::session::storages::SqliteSession;
use grammers_client::session::updates::UpdatesLike;
use grammers_client::{Client, SenderPool};
use tokio::sync::mpsc;

pub struct ClientGuard {
    pub client: Client,
    pub session: Arc<SqliteSession>,
    pub updates: mpsc::UnboundedReceiver<UpdatesLike>,
    pub rate_limiter: Arc<RateLimiter>,
    _session_lock: crate::session::SessionLock,
}

impl ClientGuard {
    pub async fn connect(
        name: &str,
        api_id: i32,
        config_path: Option<&std::path::Path>,
    ) -> anyhow::Result<Self> {
        let cfg = crate::config::load_config(config_path)?;
        let acct = cfg.accounts.get(name);
        let flood_threshold = acct
            .and_then(|a| a.flood_sleep_threshold)
            .unwrap_or(cfg.flood_sleep_threshold);
        let rate_limiter = RateLimiter::new(acct.and_then(|a| a.rpc_per_minute));
        let proxy = crate::config::proxy_url_for(&cfg, name)?;
        let locked = crate::session::open_session(name).await?;
        let session = Arc::new(locked.session);
        let pool = SenderPool::with_configuration(
            Arc::clone(&session),
            api_id,
            grammers_client::sender::ConnectionParams {
                proxy_url: proxy,
                ..Default::default()
            },
        );
        let SenderPool {
            runner,
            handle,
            updates,
        } = pool;
        tokio::spawn(runner.run());
        let conf = ClientConfiguration {
            retry_policy: Box::new(AutoSleep {
                threshold: Duration::from_secs(flood_threshold),
                io_errors_as_flood_of: Some(Duration::from_secs(1)),
            }),
            ..Default::default()
        };
        let client = Client::with_configuration(handle, conf);
        Ok(Self {
            client,
            session,
            updates,
            rate_limiter,
            _session_lock: locked.lock,
        })
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.client.disconnect();
    }
}

pub async fn authorize(client: &Client) -> TeleResult<()> {
    match client.is_authorized().await {
        Ok(true) => Ok(()),
        Ok(false) => Err(TeleError::Auth(
            "account is not logged in: run tele account login --name <name> first".to_string(),
        )),
        Err(e) => Err(crate::error::invocation_error(e)),
    }
}

pub async fn qr_login(
    client: &Client,
    updates: &mut mpsc::UnboundedReceiver<UpdatesLike>,
    creds: &crate::config::Credentials,
    mut on_token: impl FnMut(&str),
) -> anyhow::Result<()> {
    use grammers_client::tl::{self, enums};

    let receiver = std::mem::replace(updates, mpsc::unbounded_channel().1);
    let mut stream = client
        .stream_updates(
            receiver,
            UpdatesConfiguration {
                catch_up: true,
                update_queue_limit: Some(1000),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("stream updates failed: {e}"))?;

    loop {
        let response = client
            .invoke(&tl::functions::auth::ExportLoginToken {
                api_id: creds.api_id,
                api_hash: creds.api_hash.clone(),
                except_ids: Vec::new(),
            })
            .await?;

        match response {
            enums::auth::LoginToken::Token(t) => {
                let token = base64_url_encode(&t.token);
                let uri = format!("tg://login?token={token}");
                on_token(&uri);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
                loop {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, stream.next_raw()).await {
                        Ok(Ok((enums::Update::LoginToken, _, _))) => break,
                        Ok(Ok(_)) => continue,
                        Ok(Err(e)) => return Err(e.into()),
                        Err(_) => break,
                    }
                }
            }
            enums::auth::LoginToken::MigrateTo(migrate_to) => {
                let imported = client
                    .invoke_in_dc(
                        migrate_to.dc_id,
                        &tl::functions::auth::ImportLoginToken {
                            token: migrate_to.token,
                        },
                    )
                    .await?;
                match imported {
                    enums::auth::LoginToken::Success(_) => return Ok(()),
                    _ => continue,
                }
            }
            enums::auth::LoginToken::Success(_) => return Ok(()),
        }
    }
}

fn base64_url_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_url_encode_is_padless_url_safe() {
        assert_eq!(base64_url_encode(b"\xfb\xff"), "-_8");
        assert_eq!(base64_url_encode(b"f"), "Zg");
    }
}
