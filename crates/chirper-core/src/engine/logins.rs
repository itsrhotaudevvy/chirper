//! Login flows and account lifecycle.

use std::collections::BTreeMap;
use std::sync::Arc;

use chirper_bridge::model::LoginStep as BridgeStep;
use chirper_bridge::network::{LoginInput, Network};

use super::convert;
use super::{ActiveLogin, Core, Error, Result, validate};
use crate::model::{Account, AccountState, LoginState};

impl Core {
    /// `startLogin`: begin a flow and return its first step.
    pub async fn start_login(&self, network: &str, flow_id: &str) -> Result<LoginState> {
        self.inner().require_open()?;
        validate::network_id(network)?;
        validate::token(flow_id)?;
        let driver = self.driver(network)?;
        let login_id = format!("{network}_{}", crate::ids::random_hex(12));
        let ctx = self.inner().login_ctx(network, &login_id)?;
        let process = driver.start_login(flow_id, ctx.clone()).await?;
        let process_id = format!("p-{}", crate::ids::random_hex(16));
        let active = ActiveLogin {
            network: network.to_owned(),
            login_id,
            ctx,
            process: Arc::new(tokio::sync::Mutex::new(process)),
            last_step: std::sync::Mutex::new(None),
        };
        self.inner()
            .logins
            .lock()
            .expect("login map")
            .insert(process_id.clone(), active);
        self.advance(&process_id, LoginInput::start()).await
    }

    /// `submitLoginInput`: answer a user input step.
    pub async fn submit_login_input(
        &self,
        process_id: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<LoginState> {
        self.inner().require_open()?;
        validate::token(process_id)?;
        for (field, value) in values {
            validate::token(field)?;
            if value.len() > 4096 || value.chars().any(|c| c == '\0') {
                return Err(Error::InvalidInput("login value".to_owned()));
            }
        }
        self.advance(process_id, LoginInput::values(values.clone()))
            .await
    }

    /// `submitLoginCookies`: answer a cookies step.
    pub async fn submit_login_cookies(
        &self,
        process_id: &str,
        cookies: &BTreeMap<String, String>,
    ) -> Result<LoginState> {
        self.inner().require_open()?;
        validate::token(process_id)?;
        for (field, value) in cookies {
            validate::token(field)?;
            if value.len() > 16 * 1024 || value.chars().any(|c| c == '\0') {
                return Err(Error::InvalidInput("cookie value".to_owned()));
            }
        }
        self.advance(process_id, LoginInput::cookies(cookies.clone()))
            .await
    }

    /// `waitLoginStep`: blocks until a display-and-wait step is done.
    pub async fn wait_login_step(&self, process_id: &str) -> Result<LoginState> {
        self.inner().require_open()?;
        validate::token(process_id)?;
        {
            let inner = self.inner();
            let logins = inner.logins.lock().expect("login map");
            let Some(active) = logins.get(process_id) else {
                return Err(Error::NotFound);
            };
            if let Some(step) = active.last_step.lock().expect("step").clone() {
                if step.is_complete() {
                    return Ok(LoginState {
                        process_id: process_id.to_owned(),
                        step: convert::wire_step(&step),
                    });
                }
            }
        }
        self.advance(process_id, LoginInput::start()).await
    }

    /// `cancelLogin`: abandon the flow.
    pub async fn cancel_login(&self, process_id: &str) -> Result<()> {
        self.inner().require_open()?;
        validate::token(process_id)?;
        let active = self
            .inner()
            .logins
            .lock()
            .expect("login map")
            .remove(process_id);
        if let Some(active) = active {
            active.process.lock().await.cancel().await;
            remove_login_dir(&self.inner().data_dir, &active.network, &active.login_id);
        }
        Ok(())
    }

    /// `logout`: tell the network, stop the login and erase everything it stored here.
    pub async fn logout(&self, network: &str, login_id: &str) -> Result<()> {
        self.inner().require_open()?;
        validate::network_id(network)?;
        validate::token(login_id)?;
        let inner = self.inner();
        let driver = inner.runtime.network(network);
        if let Some(driver) = driver {
            if driver.supports_remote_logout() && inner.runtime.is_running(network, login_id) {
                if let Err(err) = inner.runtime.logout_remote(network, login_id).await {
                    log::debug!("{network}/{login_id}: remote logout: {err}");
                }
            }
        }
        inner.runtime.stop_login(network, login_id);
        for room_id in inner.store.purge_login(network, login_id)? {
            inner.typing.lock().expect("typing").remove(&room_id);
            inner.emit("chat_removed", serde_json::json!({ "roomId": room_id }));
        }
        if let Err(err) = inner
            .secrets
            .delete_prefix(&format!("{network}/{login_id}/"))
        {
            log::warn!("{network}/{login_id}: cannot delete credentials: {err}");
        }
        remove_login_dir(&inner.data_dir, network, login_id);
        inner.forget_login(network, login_id);
        inner.set_account(Account {
            network: network.to_owned(),
            login_id: login_id.to_owned(),
            state: AccountState::LoggedOut,
            ..Default::default()
        });
        Ok(())
    }

    /// The driver of an available network.
    fn driver(&self, network: &str) -> Result<Arc<dyn Network>> {
        let driver = self.inner().runtime.network(network).ok_or_else(|| {
            Error::Unsupported(format!("{network} is not available in this build"))
        })?;
        if let Some(reason) = driver.unavailable() {
            return Err(Error::Unsupported(reason));
        }
        Ok(driver)
    }

    /// Run one step of a process and finish the login when the driver says it is complete.
    async fn advance(&self, process_id: &str, input: LoginInput) -> Result<LoginState> {
        let (process, network, login_id, ctx) = {
            let inner = self.inner();
            let logins = inner.logins.lock().expect("login map");
            let active = logins.get(process_id).ok_or(Error::NotFound)?;
            (
                Arc::clone(&active.process),
                active.network.clone(),
                active.login_id.clone(),
                active.ctx.clone(),
            )
        };
        let step = {
            let mut guard = process.lock().await;
            guard.next(input).await?
        };
        if let Some(active) = self
            .inner()
            .logins
            .lock()
            .expect("login map")
            .get(process_id)
        {
            *active.last_step.lock().expect("step") = Some(step.clone());
        }
        let state = LoginState {
            process_id: process_id.to_owned(),
            step: convert::wire_step(&step),
        };
        if let BridgeStep::Complete { login_id: final_id } = &step {
            self.finish_login(process_id, &network, &login_id, final_id, &ctx)
                .await;
        }
        Ok(state)
    }

    /// Turn a finished flow into a stored account and a running supervisor.
    async fn finish_login(
        &self,
        process_id: &str,
        network: &str,
        provisional_id: &str,
        login_id: &str,
        ctx: &chirper_bridge::LoginCtx,
    ) {
        let inner = self.inner();
        inner.logins.lock().expect("login map").remove(process_id);
        if provisional_id != login_id {
            // The driver learned the account id during the flow; its credentials must follow it.
            if let Err(err) = inner.secrets.move_prefix(
                &format!("{network}/{provisional_id}/"),
                &format!("{network}/{login_id}/"),
            ) {
                log::warn!("{network}/{login_id}: cannot move credentials: {err}");
            }
        }
        inner.put_login(network, login_id);
        let ctx = chirper_bridge::LoginCtx {
            network: network.to_owned(),
            login_id: login_id.to_owned(),
            data_dir: ctx.data_dir.clone(),
            secrets: ctx.secrets.clone(),
            http: chirper_bridge::http_client(),
        };
        inner.runtime.start_login(network, login_id, ctx);
        log::info!("{network}/{login_id}: login complete");
    }
}

impl std::fmt::Debug for ActiveLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveLogin")
            .field("network", &self.network)
            .field("login_id", &self.login_id)
            .finish_non_exhaustive()
    }
}

/// Delete the private directory of a login.
fn remove_login_dir(data_dir: &std::path::Path, network: &str, login_id: &str) {
    let dir = data_dir
        .join("logins")
        .join(format!("{network}.{login_id}"));
    if let Err(err) = std::fs::remove_dir_all(&dir) {
        if err.kind() != std::io::ErrorKind::NotFound {
            log::warn!("cannot delete {dir:?}: {err}");
        }
    }
}
