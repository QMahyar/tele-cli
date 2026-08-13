use std::sync::Arc;
use std::time::Duration;

use grammers_client::client::{AutoSleep, ClientConfiguration, UpdatesConfiguration};
use grammers_client::session::updates::UpdatesLike;
use grammers_client::{Client, SenderPool};
use tokio::sync::mpsc;

pub struct ClientGuard {
    pub client: Client,
    pub updates: mpsc::UnboundedReceiver<UpdatesLike>,
}

impl ClientGuard {
    pub async fn connect(
        name: &str,
        api_id: i32,
        config_path: Option<&std::path::Path>,
    ) -> anyhow::Result<Self> {
        let cfg = crate::config::load_config(config_path)?;
        let proxy = crate::config::proxy_url_for(&cfg, name)?;
        let session = Arc::new(crate::session::open_session(name).await?);
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
                threshold: Duration::from_secs(cfg.flood_sleep_threshold),
                io_errors_as_flood_of: Some(Duration::from_secs(1)),
            }),
            ..Default::default()
        };
        let client = Client::with_configuration(handle, conf);
        Ok(Self { client, updates })
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.client.disconnect();
    }
}

pub async fn authorize(client: &Client, _creds: &crate::config::Credentials) -> anyhow::Result<()> {
    if client.is_authorized().await? {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "account is not logged in: run tele account login --name <name> first"
    ))
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
        .stream_updates(receiver, UpdatesConfiguration::default())
        .await
        .map_err(|e| anyhow::anyhow!("stream updates failed: {e}"))?;

    let mut last_token: Option<Vec<u8>> = None;

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
                last_token = Some(t.token.clone());
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
            enums::auth::LoginToken::MigrateTo(_) => {
                let Some(bytes) = last_token.clone() else {
                    return Err(anyhow::anyhow!(
                        "login token migration requested before a token was issued"
                    ));
                };
                let imported = client
                    .invoke(&tl::functions::auth::ImportLoginToken { token: bytes })
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
