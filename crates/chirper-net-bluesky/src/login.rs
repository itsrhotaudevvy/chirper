//! The `password` login flow: a handle or e-mail address, an app password and, when the account asks
//! for one, a second-factor code. The login id is the account's DID; the engine moves the stored
//! session from the provisional id to it.

use async_trait::async_trait;
use zeroize::Zeroizing;

use chirper_bridge::model::{InputField, LoginStep};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Result};

use crate::Hosts;
use crate::account::{self, Stored};
use crate::api::types::AuthSession;
use crate::api::{ApiError, Pds, identity, server};
use crate::ids;

/// The step ids of the flow, as the Go bridge names them.
const PASSWORD_STEP: &str = "fi.mau.bluesky.password";
const CODE_STEP: &str = "code";
/// The server a login uses when the form leaves it empty.
const DEFAULT_SERVER: &str = "bsky.social";

/// A sign-in that waits for its second-factor code.
struct Pending {
    identifier: String,
    password: Zeroizing<String>,
    pds: String,
}

/// One password login.
pub struct PasswordLogin {
    ctx: LoginCtx,
    hosts: Hosts,
    pending: Option<Pending>,
    done: bool,
}

impl PasswordLogin {
    pub fn new(ctx: LoginCtx, hosts: Hosts) -> Self {
        Self {
            ctx,
            hosts,
            pending: None,
            done: false,
        }
    }

    /// The first step: the form.
    fn password_step() -> LoginStep {
        LoginStep::UserInput {
            step_id: PASSWORD_STEP.to_owned(),
            instructions: "Use an app password, not your account password. Create one under \
                           Settings, Privacy and security, App passwords."
                .to_owned(),
            fields: vec![
                field(
                    "domain",
                    "Server",
                    "text",
                    "The Bluesky server. Leave empty for bsky.social.",
                ),
                field(
                    "username",
                    "Username or email",
                    "text",
                    "Bluesky handle or e-mail address.",
                ),
                field(
                    "password",
                    "Password",
                    "password",
                    "An app password is recommended.",
                ),
            ],
        }
    }

    /// The second-factor step.
    fn code_step() -> LoginStep {
        LoginStep::UserInput {
            step_id: CODE_STEP.to_owned(),
            instructions: "This account asks for a second-factor code. Bluesky sent it to the \
                           account's e-mail address."
                .to_owned(),
            fields: vec![field("code", "Code", "text", "The code from the e-mail.")],
        }
    }

    /// Sign in and store the session; the login is complete from here.
    fn finish(&mut self, session: AuthSession, pds: String) -> Result<LoginStep> {
        let did = session.did.clone();
        let pds = session
            .did_doc
            .as_ref()
            .and_then(|doc| identity::pds_from_doc(doc, &self.hosts.scheme))
            .unwrap_or(pds);
        let stored = Stored::from_session(session, pds);
        account::store(&self.ctx, &stored)?;
        self.done = true;
        Ok(LoginStep::Complete {
            login_id: ids::login_id(&did),
        })
    }
}

fn field(id: &str, name: &str, kind: &str, description: &str) -> InputField {
    InputField {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind.to_owned(),
        description: description.to_owned(),
        options: Vec::new(),
        pattern: String::new(),
    }
}

/// The server a login reaches: the form's domain over the driver's scheme.
fn server_origin(hosts: &Hosts, domain: &str) -> String {
    format!("{}://{domain}", hosts.scheme)
}

/// Whether a server name is a plain host with an optional port.
fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

/// The PDS to sign in at. A handle is resolved through the server and its DID document; an e-mail
/// address signs in at the server itself.
async fn sign_in_origin(
    ctx: &LoginCtx,
    hosts: &Hosts,
    domain: &str,
    identifier: &str,
) -> Result<String> {
    let origin = server_origin(hosts, domain);
    if identifier.contains('@') {
        return Ok(origin);
    }
    let pds = Pds::new(ctx.http.clone(), origin.clone());
    let did = identity::resolve_handle(&pds, identifier)
        .await
        .map_err(ApiError::into_error)?;
    Ok(identity::pds_for(&ctx.http, hosts, &did)
        .await?
        .unwrap_or(origin))
}

fn refused(err: ApiError) -> Error {
    match err {
        ApiError::Other(Error::Auth(_)) => {
            Error::Auth("Bluesky refused the username or password".to_owned())
        }
        other => other.into_error(),
    }
}

fn required(input: &LoginInput, field: &str) -> Result<String> {
    input
        .values
        .get(field)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Auth(format!("the {field} is empty")))
}

#[async_trait]
impl LoginProcess for PasswordLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if let Some(pending) = self.pending.take() {
            let code = required(&input, "code")?;
            let pds = pending.pds.clone();
            let session = server::create_session(
                &Pds::new(self.ctx.http.clone(), pds.clone()),
                &pending.identifier,
                &pending.password,
                Some(&code),
            )
            .await
            .map_err(refused)?;
            return self.finish(session, pds);
        }
        if input.values.is_empty() {
            return Ok(Self::password_step());
        }
        let domain = input
            .values
            .get("domain")
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_SERVER.to_owned());
        if !valid_domain(&domain) {
            return Err(Error::Auth("the server name is not a host name".to_owned()));
        }
        let identifier = required(&input, "username")?
            .trim_start_matches('@')
            .to_owned();
        let password = Zeroizing::new(required(&input, "password")?);

        let pds = sign_in_origin(&self.ctx, &self.hosts, &domain, &identifier).await?;
        let attempt = server::create_session(
            &Pds::new(self.ctx.http.clone(), pds.clone()),
            &identifier,
            &password,
            None,
        )
        .await;
        match attempt {
            Ok(session) => self.finish(session, pds),
            Err(ApiError::AuthFactorRequired) => {
                self.pending = Some(Pending {
                    identifier,
                    password,
                    pds,
                });
                Ok(Self::code_step())
            }
            Err(err) => Err(refused(err)),
        }
    }

    async fn cancel(&mut self) {
        self.pending = None;
    }
}
