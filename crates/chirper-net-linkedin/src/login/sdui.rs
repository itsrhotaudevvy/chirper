//! The server-driven UI of LinkedIn's login page: the bootstrap page carries the password action
//! in its hydration data, the driver posts that action back with the credentials, and the answer
//! is a Flight-style record (`0:{json}`) that says where to go next.
//!
//! Everything here reads data from the network as data: every lookup is a `Value` probe that
//! returns `None` on a shape it does not expect. Reference: `linkedingo/login_sdui.go`.

use serde_json::{Map, Value, json};

use super::encrypt::{Encryption, find_encryption};
use super::html::{self, Token};

/// The request id of the password submission.
pub(super) const AUTH_REQUEST: &str = "com.linkedin.sdui.requests.login.authenticate";
/// The request id that turns an inline challenge route into a checkpoint.
pub(super) const CHALLENGE_ROUTE_REQUEST: &str =
    "com.linkedin.sdui.requests.challenges.routeToChallengeRequest";
/// The screen the password form lives on, when the page does not name one.
const DEFAULT_SCREEN: &str = "com.linkedin.sdui.flagshipnav.login.Login";
/// The longest proof value the completion step accepts.
const MAX_PROOF_LEN: usize = 8192;
/// How deep the parsers look into the hydration tree.
const MAX_DEPTH: usize = 100;

/// A password action found in the bootstrap page.
pub(super) struct Page {
    /// The server request, as the page carries it.
    action: Value,
    /// The arguments of the request, including the state keys the page asks for.
    arguments: Value,
    /// The payload, with the bindings of the identifier and password fields.
    payload: Value,
    /// The screen the request belongs to.
    screen_id: String,
    /// The public key fields are encrypted with, when the page asks for it.
    encryption: Option<Encryption>,
}

impl Page {
    /// Names the screen the page is on, as the bootstrap response does.
    pub(super) fn set_screen(&mut self, screen: String) {
        self.screen_id = screen;
    }
}

/// The automatic request that exchanges a passed checkpoint for a session.
pub(super) struct Completion {
    /// The server request to post back.
    pub(super) action: Value,
    /// The screen the request belongs to.
    pub(super) screen_id: String,
}

/// What the server answered to a submission.
pub(super) struct Response {
    /// The answer carries errors.
    pub(super) has_errors: bool,
    /// The answer repeats a credential error inline, in the form.
    pub(super) inline_error: bool,
    /// Where the answer sends the browser next.
    pub(super) completion_urls: Vec<String>,
    /// An inline challenge route to post back, when there is one.
    pub(super) challenge_route: Option<Value>,
}

/// The bindings of one payload field: the state key the server reads it from.
struct Binding {
    key: String,
    namespace: String,
    encrypted: bool,
}

/// Reads the bootstrap page. `None` when the password action is not there.
pub(super) fn parse_page(body: &[u8]) -> Option<Page> {
    let rows = hydration(body)?;
    let mut page = None;
    let mut encryption = None;
    for (_, value) in &rows {
        if page.is_none() {
            page = find_password_action(value, 0);
        }
        if encryption.is_none() {
            encryption = find_encryption(value, 0);
        }
    }
    let mut page = page?;
    page.encryption = encryption;
    Some(page)
}

/// Finds the automatic completion request in a page that follows a passed checkpoint.
pub(super) fn parse_completion(body: &[u8]) -> Option<Completion> {
    hydration(body)?
        .iter()
        .find_map(|(_, value)| find_completion(value, 0))
}

/// The submission for a page: the identifier and password in the page's own state keys.
pub(super) fn page_request(page: &Page, identifier: &str, password: &str) -> Option<Value> {
    let mut states = Vec::with_capacity(4);
    for (name, value) in [
        ("rememberMeOptInCheckboxState", "Checked"),
        ("apfc", ""),
        ("identifier", identifier),
        ("password", password),
    ] {
        let binding = binding(page, name)?;
        let value = if binding.encrypted {
            page.encryption.as_ref()?.encrypt(value)?
        } else {
            value.to_owned()
        };
        let mut state = json!({
            "key": binding.key,
            "namespace": binding.namespace,
            "value": value,
            "originalProtoCase": "stringValue",
            "protoKey": {
                "$type": "proto.sdui.Key",
                "value": { "$case": "id", "id": binding.key },
            },
        });
        if name == "apfc" {
            state["value"] = json!("");
            state.as_object_mut()?.remove("originalProtoCase");
        }
        states.push(state);
    }
    Some(marshal_action(&page.action, states, &page.screen_id))
}

/// The body of a server request: the action with its states and screen filled in.
pub(super) fn marshal_action(action: &Value, states: Vec<Value>, screen_id: &str) -> Value {
    let mut arguments = action
        .get("requestedArguments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    arguments.insert("states".to_owned(), Value::Array(states.clone()));
    arguments.insert("screenId".to_owned(), json!(screen_id));
    arguments.insert("knownTemplateIds".to_owned(), json!([]));
    json!({
        "requestId": action.get("requestId").cloned().unwrap_or(Value::Null),
        "serverRequest": action,
        "states": states,
        "requestedArguments": arguments,
    })
}

/// The screen the login page is on: the bootstrap's SDUI screen, or the login screen.
pub(super) fn login_screen(leaf_screen: Option<&str>) -> String {
    leaf_screen
        .filter(|screen| screen.starts_with("com.linkedin.sdui."))
        .unwrap_or(DEFAULT_SCREEN)
        .to_owned()
}

/// Reads the `0:` record of a submission answer. `None` when there is no record or no envelope.
pub(super) fn parse_response(body: &[u8]) -> Option<Response> {
    let text = String::from_utf8_lossy(body);
    let record = text
        .split('\n')
        .find_map(|line| line.split_once(':').filter(|(id, _)| *id == "0"))?
        .1;
    let data: Value = serde_json::from_str(record).ok()?;
    let response = data.get("response")?.as_object()?;
    let has_errors = match response.get("errors") {
        None | Some(Value::Null) => false,
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => return None,
    };
    let actions = completion_actions(response);
    let inline_error = actions.iter().any(|action| {
        action.get("$type") == Some(&json!("proto.sdui.actions.core.ReplaceComponent"))
            && has_inline_error(
                action
                    .pointer("/value/content/newComponent")
                    .unwrap_or(&Value::Null),
                false,
                0,
            )
    });
    let completion_urls = actions
        .iter()
        .filter_map(|action| action.pointer("/value/content/url/urlValue/url"))
        .filter_map(Value::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect();
    Some(Response {
        has_errors,
        inline_error,
        completion_urls,
        challenge_route: find_challenge_route(response),
    })
}

/// The completion actions of a response envelope.
fn completion_actions(response: &Map<String, Value>) -> Vec<&Value> {
    response
        .get("completionAction")
        .and_then(|completion| completion.get("actions"))
        .and_then(Value::as_array)
        .map(|actions| actions.iter().collect())
        .unwrap_or_default()
}

/// The hydration rows of the bootstrap page, as `(id, value)` pairs. Each row is one line of a
/// chunk in `window.__como_rehydration__ = [...]`; rows that are not JSON are other Flight rows.
fn hydration(body: &[u8]) -> Option<Vec<(String, Value)>> {
    let page = String::from_utf8_lossy(body);
    for token in html::tokens(&page) {
        let Token::Start { name, text, .. } = &token else {
            continue;
        };
        if name != "script" || token.attr("id") != Some("rehydrate-data") {
            continue;
        }
        let Some(assignment) = text
            .trim()
            .strip_prefix("window.__como_rehydration__")
            .and_then(|rest| rest.trim_start().strip_prefix('='))
        else {
            continue;
        };
        let chunks: Vec<String> = serde_json::Deserializer::from_str(assignment.trim_start())
            .into_iter()
            .next()?
            .ok()?;
        let rows = chunks
            .iter()
            .flat_map(|chunk| chunk.split('\n'))
            .filter_map(|row| {
                let (id, data) = row.split_once(':')?;
                let value = serde_json::from_str(data).ok()?;
                Some((id.to_owned(), value))
            })
            .collect();
        return Some(rows);
    }
    None
}

/// Finds the password action: the request with the password authentication type, not the
/// profile login, and bindings for both fields.
fn find_password_action(value: &Value, depth: usize) -> Option<Page> {
    if depth > MAX_DEPTH {
        return None;
    }
    match value {
        Value::Object(map) => {
            let arguments = object_or_empty(map.get("requestedArguments"));
            let payload = object_or_empty(arguments.get("payload"));
            if map.get("requestId") == Some(&json!(AUTH_REQUEST))
                && payload.get("authenticationType") == Some(&json!("AuthenticationType_PASSWORD"))
                && payload.get("isLoginWithProfile") == Some(&Value::Bool(false))
            {
                let page = Page {
                    action: value.clone(),
                    arguments: Value::Object(arguments.clone()),
                    payload: Value::Object(payload.clone()),
                    screen_id: DEFAULT_SCREEN.to_owned(),
                    encryption: None,
                };
                if binding(&page, "identifier").is_some() && binding(&page, "password").is_some() {
                    return Some(page);
                }
            }
            map.values()
                .find_map(|child| find_password_action(child, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_password_action(child, depth + 1)),
        _ => None,
    }
}

/// Finds the automatic request of a checkpoint completion: a SDUI screen whose `onAppear` posts
/// the authentication with a passed checkpoint proof and no state keys.
fn find_completion(value: &Value, depth: usize) -> Option<Completion> {
    if depth > MAX_DEPTH {
        return None;
    }
    match value {
        Value::Object(map) => {
            if let Some(screen) = map
                .get("screenId")
                .and_then(Value::as_str)
                .filter(|screen| screen.starts_with("com.linkedin.sdui."))
            {
                let actions = map
                    .get("onAppear")
                    .and_then(|appear| appear.get("actions"))
                    .and_then(Value::as_array);
                for wrapper in actions.into_iter().flatten() {
                    if let Some(completion) = completion_action(wrapper, screen) {
                        return Some(completion);
                    }
                }
            }
            map.values()
                .find_map(|child| find_completion(child, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_completion(child, depth + 1)),
        _ => None,
    }
}

/// The completion in one `onAppear` action, when it is the exact automatic request.
fn completion_action(wrapper: &Value, screen: &str) -> Option<Completion> {
    let action = wrapper.get("value")?;
    let arguments = object_or_empty(action.get("requestedArguments"));
    let payload = object_or_empty(arguments.get("payload"));
    let valid_proof = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| {
                !value.is_empty() && value.len() <= MAX_PROOF_LEN && !value.starts_with('$')
            })
    };
    let no_states = arguments
        .get("requestedStateKeys")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    let proof_ok = valid_proof("chpToken")
        && (valid_proof("vcd")
            || (valid_proof("challengeId") && valid_proof("encryptedRecognizedDeviceFlag")));
    let exact = wrapper.get("$type") == Some(&json!("proto.sdui.actions.core.ServerRequest"))
        && action.get("requestId") == Some(&json!(AUTH_REQUEST))
        && payload.get("authenticationType") == Some(&json!("AuthenticationType_UNKNOWN"))
        && no_states
        && proof_ok;
    exact.then(|| Completion {
        action: action.clone(),
        screen_id: screen.to_owned(),
    })
}

/// The bindings of one payload field, when the server names a memory namespace for it.
fn binding(page: &Page, name: &str) -> Option<Binding> {
    let field = page.payload.get(name)?.as_object()?;
    let key = field.get("key")?.as_str()?;
    let namespace = field.get("namespace")?.as_str()?;
    if key.is_empty() || key.len() > 128 || namespace != "MemoryNamespace" {
        return None;
    }
    let mut encrypted = field.get("isEncrypted") == Some(&Value::Bool(true));
    for state in page
        .arguments
        .get("requestedStateKeys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let state_key = state.pointer("/key/value");
        let matches = state_key.is_some_and(|key_value| {
            key_value.get("$case") == Some(&json!("id")) && key_value.get("id") == Some(&json!(key))
        });
        if matches && state.get("isEncrypted") == Some(&Value::Bool(true)) {
            encrypted = true;
        }
    }
    Some(Binding {
        key: key.to_owned(),
        namespace: namespace.to_owned(),
        encrypted,
    })
}

/// The challenge route of a response: a server request whose flow names a challenge.
fn find_challenge_route(response: &Map<String, Value>) -> Option<Value> {
    completion_actions(response)
        .into_iter()
        .find_map(|wrapper| {
            let action = wrapper.get("value")?;
            let flow = action.pointer("/requestedArguments/payload/flowData")?;
            let exact = wrapper.get("$type")
                == Some(&json!("proto.sdui.actions.core.ServerRequest"))
                && action.get("requestId") == Some(&json!(CHALLENGE_ROUTE_REQUEST))
                && is_inline_text(flow.get("challengeId").unwrap_or(&Value::Null));
            exact.then(|| action.clone())
        })
}

/// Whether an inline credential error sits in the form (the server reports it there, not as an
/// error envelope).
fn has_inline_error(value: &Value, feedback: bool, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match value {
        Value::Object(map) => {
            let feedback = feedback
                || map
                    .get("viewTrackingSpecs")
                    .and_then(|specs| specs.get("viewName"))
                    == Some(&json!("identifier-password-inline-feedback"));
            let children = map
                .get("textProps")
                .and_then(|props| props.get("children"))
                .and_then(Value::as_array);
            if feedback && children.is_some_and(|items| items.iter().any(is_inline_text)) {
                return true;
            }
            map.values()
                .any(|child| has_inline_error(child, feedback, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .any(|child| has_inline_error(child, feedback, depth + 1)),
        Value::String(text) => feedback && text == "Wrong email or password.",
        _ => false,
    }
}

/// Non-blank text that is not an unresolved `$` reference.
fn is_inline_text(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| !text.trim().is_empty() && !text.starts_with('$'))
}

/// An object, or an empty one, so lookups below it never fail.
fn object_or_empty(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bootstrap page with the password action and, optionally, encryption metadata.
    fn bootstrap(encrypt_password: bool) -> Vec<u8> {
        let mut payload = json!({
            "authenticationType": "AuthenticationType_PASSWORD",
            "isLoginWithProfile": false,
            "bcookie": "from-bootstrap",
            "bscookie": "server-action-cookie",
        });
        let mut keys = Vec::new();
        for field in [
            "rememberMeOptInCheckboxState",
            "apfc",
            "identifier",
            "password",
        ] {
            let encrypted = field == "password" && encrypt_password;
            payload[field] =
                json!({ "key": format!("{field}-state"), "namespace": "MemoryNamespace" });
            keys.push(json!({
                "key": { "value": { "$case": "id", "id": format!("{field}-state") } },
                "isEncrypted": encrypted,
            }));
        }
        let action = json!({
            "requestId": AUTH_REQUEST,
            "requestedArguments": {
                "$type": "proto.sdui.actions.requests.RequestedArguments",
                "payload": payload,
                "requestedStateKeys": keys,
                "requestMetadata": { "$type": "proto.sdui.common.RequestMetadata" },
            },
            "isApfcEnabled": false,
            "isStreaming": false,
        });
        let row = json!(["$", "div", null, { "onClick": action }]);
        let rows = vec!["0:I[123]\n".to_owned(), format!("1:{row}\n")];
        let chunks = serde_json::to_string(&rows).unwrap_or_default();
        format!(
            r#"<html><script id="rehydrate-data">window.__como_rehydration__ = {chunks}</script></html>"#
        )
        .into_bytes()
    }

    /// A submission answer that redirects to `destination`.
    fn answer(destination: &str) -> String {
        let data = json!({
            "states": [],
            "response": {
                "errors": [],
                "isRetryable": false,
                "completionAction": {
                    "actions": [{ "value": { "content": { "url": { "urlValue": { "url": destination } } } } }]
                }
            }
        });
        format!("1:[]\n0:{data}\n")
    }

    #[test]
    fn the_password_action_is_found_and_its_request_is_built() {
        let page = parse_page(&bootstrap(false)).expect("the page has the action");
        let body = page_request(&page, "alice@example.invalid", " spaces \" remain ")
            .expect("the request builds");
        let states = body["states"].as_array().expect("states");
        assert_eq!(states.len(), 4);
        assert_eq!(states[0]["value"], "Checked");
        assert_eq!(states[1]["value"], "");
        assert!(states[1].get("originalProtoCase").is_none());
        assert_eq!(states[2]["value"], "alice@example.invalid");
        assert_eq!(states[3]["value"], " spaces \" remain ");
        assert_eq!(states[3]["originalProtoCase"], "stringValue");
        assert_eq!(body["requestedArguments"]["states"], body["states"]);
        assert_eq!(
            body["requestedArguments"]["payload"]["bscookie"],
            "server-action-cookie"
        );
    }

    #[test]
    fn a_page_without_the_action_is_rejected() {
        assert!(parse_page(b"<html>nothing here</html>").is_none());
        assert!(
            parse_page(
                b"<script id=\"rehydrate-data\">window.__como_rehydration__ = [not json]</script>"
            )
            .is_none()
        );
    }

    #[test]
    fn a_success_answer_names_the_feed() {
        let response = parse_response(answer("/feed/").as_bytes()).expect("parses");
        assert!(!response.has_errors && !response.inline_error);
        assert_eq!(response.completion_urls, vec!["/feed/".to_owned()]);
        assert!(response.challenge_route.is_none());
    }

    #[test]
    fn inline_and_envelope_errors_are_seen() {
        let inline = r#"0:{"response":{"errors":[],"completionAction":{"actions":[{"$type":"proto.sdui.actions.core.ReplaceComponent","value":{"content":{"newComponent":["$","div",null,{"viewTrackingSpecs":{"viewName":"identifier-password-inline-feedback"},"children":{"textProps":{"children":["Wrong email or password."]}}}]}}}]}}}"#;
        let response = parse_response(inline.as_bytes()).expect("parses");
        assert!(response.inline_error);
        let errors = r#"0:{"response":{"errors":[{"message":"x"}]}}"#;
        assert!(
            parse_response(errors.as_bytes())
                .expect("parses")
                .has_errors
        );
        assert!(parse_response(b"0:{\"secret\":true}").is_none());
        assert!(parse_response(b"1:[]").is_none());
    }

    #[test]
    fn the_completion_must_be_the_exact_automatic_request() {
        let root = json!({
            "screenId": "com.linkedin.sdui.flagshipnav.login.Login",
            "onAppear": { "actions": [{
                "$type": "proto.sdui.actions.core.ServerRequest",
                "value": {
                    "requestId": AUTH_REQUEST,
                    "requestedArguments": {
                        "requestedStateKeys": [],
                        "payload": {
                            "authenticationType": "AuthenticationType_UNKNOWN",
                            "chpToken": "checkpoint-proof",
                            "vcd": "verification-proof",
                        }
                    }
                }
            }] }
        });
        let rows = vec![format!("0:{root}\n")];
        let page = format!(
            r#"<script id="rehydrate-data">window.__como_rehydration__ = {}</script>"#,
            serde_json::to_string(&rows).unwrap_or_default()
        );
        assert!(parse_completion(page.as_bytes()).is_some());
        let wrong_type = page.replace("AuthenticationType_UNKNOWN", "AuthenticationType_PASSWORD");
        assert!(parse_completion(wrong_type.as_bytes()).is_none());
        let no_proof = page.replace("checkpoint-proof", "");
        assert!(parse_completion(no_proof.as_bytes()).is_none());
        let unresolved = page.replace("verification-proof", "$unresolved");
        assert!(parse_completion(unresolved.as_bytes()).is_none());
    }
}
