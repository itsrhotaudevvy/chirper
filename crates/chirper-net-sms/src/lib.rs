//! The SMS driver for Chirper's unified bridge runtime.
//!
//! On Android the app provides an [`SmsPlatform`] that reads the phone's own SMS and MMS provider
//! and sends through `SmsManager`. Chirper is not the default SMS app, so this driver can read,
//! send and observe changes, but it cannot mark messages read, delete them or write to the
//! provider. There is no SMS API on iOS or on the desktops, so the network is listed there with
//! its reason and nothing else.

#![forbid(unsafe_code)]

mod map;
mod platform;
mod session;

#[cfg(test)]
mod testing;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, InputField, LoginFlow, LoginStep};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

pub use platform::{
    MmsPart, SmsAddress, SmsMessage, SmsPermissions, SmsPlatform, SmsPlatformEvent, SmsThread,
};

/// The network id used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "sms";
/// The login id of the phone's own SMS service. There is one per device.
pub const DEVICE_LOGIN_ID: &str = "device";
/// Why SMS is unavailable where there is no platform provider.
const UNAVAILABLE: &str =
    "SMS works in the Android app, which reads and sends through the phone's own SMS service.";

/// The SMS driver. Without a platform it is listed as unavailable.
#[derive(Clone, Default)]
pub struct Sms {
    platform: Option<Arc<dyn SmsPlatform>>,
}

impl fmt::Debug for Sms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sms")
            .field("available", &self.platform.is_some())
            .finish()
    }
}

impl Sms {
    /// The driver with no platform: listed as unavailable.
    pub fn new() -> Self {
        Self { platform: None }
    }

    /// The driver on a platform that provides SMS.
    pub fn with_platform(platform: Arc<dyn SmsPlatform>) -> Self {
        Self {
            platform: Some(platform),
        }
    }

    fn platform(&self, action: &'static str) -> Result<Arc<dyn SmsPlatform>> {
        self.platform.clone().ok_or(Error::Unsupported(action))
    }
}

#[async_trait]
impl Network for Sms {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "SMS"
    }

    fn color(&self) -> &'static str {
        "#8E8E93"
    }

    fn capabilities(&self) -> Capabilities {
        // Plain text in both directions only: no edits, reactions, replies, receipts or media
        // through the runtime yet.
        Capabilities::default()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![LoginFlow {
            id: DEVICE_LOGIN_ID.to_owned(),
            name: "This phone".to_owned(),
            description: "Read and send SMS with this phone's SIM.".to_owned(),
        }]
    }

    fn unavailable(&self) -> Option<String> {
        match self.platform {
            Some(_) => None,
            None => Some(UNAVAILABLE.to_owned()),
        }
    }

    async fn start_login(&self, flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        if flow_id != DEVICE_LOGIN_ID {
            return Err(Error::Unsupported("this login flow"));
        }
        Ok(Box::new(DeviceLogin {
            platform: self.platform("logging in to SMS")?,
            shown: false,
            done: false,
        }))
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let platform = self.platform("connecting to SMS")?;
        Ok(Box::new(session::SmsSession::open(platform, ctx).await?))
    }

    fn supports_remote_logout(&self) -> bool {
        false
    }
}

/// The one-step login of this phone: ask for the permissions, then complete.
struct DeviceLogin {
    platform: Arc<dyn SmsPlatform>,
    shown: bool,
    done: bool,
}

#[async_trait]
impl LoginProcess for DeviceLogin {
    async fn next(&mut self, _input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if !self.shown {
            self.shown = true;
            return Ok(LoginStep::UserInput {
                step_id: "permissions".to_owned(),
                instructions: "Chirper reads this phone's text messages to show them, and sends \
                    new ones through its SIM. Nothing is sent to a Chirper server."
                    .to_owned(),
                fields: vec![InputField {
                    id: "allow".to_owned(),
                    name: "Allow SMS access".to_owned(),
                    kind: "button".to_owned(),
                    description: "Read and send text messages. Contact names are read when \
                        you allow contacts."
                        .to_owned(),
                    options: Vec::new(),
                    pattern: String::new(),
                }],
            });
        }
        let platform = self.platform.clone();
        let granted = session::blocking(move || platform.request_permissions()).await?;
        if !granted.can_chat() {
            return Err(Error::Auth("SMS permission was not granted".to_owned()));
        }
        self.done = true;
        Ok(LoginStep::Complete {
            login_id: DEVICE_LOGIN_ID.to_owned(),
        })
    }
}
