use std::future::Future;

use crate::client::{self, ClientGuard, ServeShares};
use crate::commands::credentials::creds_api_id;
use crate::error::TeleResult;
use crate::executor::{finish, run_fanout, GlobalFlags};
use crate::output::Envelope;

pub async fn run_with_client<F, Fut>(
    flags: &GlobalFlags,
    dry_payload: impl Fn() -> TeleResult<serde_json::Value> + Send + Sync + Clone + 'static,
    core: F,
) -> TeleResult<Envelope>
where
    F: Fn(ServeShares) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = TeleResult<serde_json::Value>> + Send + 'static,
{
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let dry_payload = dry_payload.clone();
        let core = core.clone();
        Box::pin(async move {
            if dry_run {
                return dry_payload();
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            core(guard.shares()).await
        })
    })
    .await
}

pub async fn with_client<F, Fut>(
    flags: &GlobalFlags,
    dry_payload: impl Fn() -> TeleResult<serde_json::Value> + Send + Sync + Clone + 'static,
    core: F,
) -> TeleResult<i32>
where
    F: Fn(ServeShares) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = TeleResult<serde_json::Value>> + Send + 'static,
{
    let envelope = run_with_client(flags, dry_payload, core).await?;
    finish(flags, &envelope)
}

#[allow(dead_code)]
pub async fn run_with_client_params<P, F, Fut>(
    flags: &GlobalFlags,
    params: P,
    dry_payload: impl Fn(&P) -> TeleResult<serde_json::Value> + Send + Sync + Clone + 'static,
    core: F,
) -> TeleResult<Envelope>
where
    P: Clone + Send + Sync + 'static,
    F: Fn(ServeShares, P) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = TeleResult<serde_json::Value>> + Send + 'static,
{
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let params = params.clone();
        let dry_payload = dry_payload.clone();
        let core = core.clone();
        Box::pin(async move {
            if dry_run {
                return dry_payload(&params);
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            core(guard.shares(), params).await
        })
    })
    .await
}

#[allow(dead_code)]
pub async fn with_client_params<P, F, Fut>(
    flags: &GlobalFlags,
    params: P,
    dry_payload: impl Fn(&P) -> TeleResult<serde_json::Value> + Send + Sync + Clone + 'static,
    core: F,
) -> TeleResult<i32>
where
    P: Clone + Send + Sync + 'static,
    F: Fn(ServeShares, P) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = TeleResult<serde_json::Value>> + Send + 'static,
{
    let envelope = run_with_client_params(flags, params, dry_payload, core).await?;
    finish(flags, &envelope)
}


