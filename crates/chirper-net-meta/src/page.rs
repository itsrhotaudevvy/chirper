//! The Messenger web bootstrap: the page's embedded `__bbox` modules, read for the handful of
//! configuration values the realtime protocol needs.
//!
//! The reference (`pkg/messagix/httpclient/js_module_parser.go` and `modules.go`, AGPL-3.0) walks
//! every script tag of the page, follows `ScheduledServerJS` wrappers and copies each `define`d
//! config into a typed table. The port keeps the same walk but only retains the values it reads: the
//! logged-in user, the Messenger application ids, the device id, the sync parameters and the lightspeed
//! protocol version.

use std::collections::BTreeMap;

use chirper_bridge::Error;
use serde_json::Value;

/// The browser user agent every page and socket request presents.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// The values the realtime protocol needs from the bootstrap page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bootstrap {
    /// The logged-in account id, as the page reports it (`USER_ID`).
    pub user_id: String,
    /// The account's display name (`NAME`).
    pub name: String,
    /// The application id of lightspeed requests (`CurrentUserInitialData.APP_ID`).
    pub app_id: String,
    /// The application id of the realtime socket (`DGWWebConfig.appId`).
    pub socket_app_id: String,
    /// The web device id the socket identifies with (`MqttWebDeviceID.clientID`).
    pub device_id: String,
    /// The mailbox sync parameters of database 1 (`LSPlatformMessengerSyncParams.mailbox`).
    pub mailbox_sync_params: String,
    /// The protocol version of lightspeed requests; zero when the page did not carry it.
    pub version_id: i64,
}

/// Parse a bootstrap page. A page that shows no logged-in user is an authentication failure.
pub fn parse(html: &str) -> Result<Bootstrap, Error> {
    if html.contains("\"USER_ID\":\"0\"") {
        return Err(Error::Auth("the page shows no logged-in user".to_owned()));
    }
    let mut configs = BTreeMap::new();
    let mut versions = Vec::new();
    for content in script_contents(html) {
        let Ok(value) = serde_json::from_str::<Value>(content) else {
            continue;
        };
        walk_bbox(&value, &mut configs, &mut versions);
    }
    let user = configs.get("CurrentUserInitialData");
    let user_id = string_field(user, "USER_ID");
    if user_id.is_empty() {
        return Err(Error::Protocol(
            "the bootstrap page does not name the logged-in user".to_owned(),
        ));
    }
    let sync = configs.get("LSPlatformMessengerSyncParams");
    Ok(Bootstrap {
        user_id,
        name: string_field(user, "NAME"),
        app_id: string_field(user, "APP_ID"),
        socket_app_id: string_field(configs.get("DGWWebConfig"), "appId"),
        device_id: string_field(configs.get("MqttWebDeviceID"), "clientID"),
        mailbox_sync_params: string_field(sync, "mailbox"),
        version_id: versions.into_iter().max().unwrap_or(0),
    })
}

fn string_field(object: Option<&Value>, key: &str) -> String {
    object
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The text of every `<script ...>...</script>` element, without its attributes.
fn script_contents(html: &str) -> Vec<&str> {
    let mut contents = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let body = &after[open_end + 1..];
        let Some(close) = body.find("</script>") else {
            break;
        };
        contents.push(&body[..close]);
        rest = &body[close..];
    }
    contents
}

/// Walk one parsed script: `__bbox` objects hold `require` and `define` entries, and a
/// `ScheduledServerJS` wrapper holds more of them.
fn walk_bbox(value: &Value, configs: &mut BTreeMap<String, Value>, versions: &mut Vec<i64>) {
    let Some(bbox) = value.get("__bbox") else {
        return;
    };
    for entry in entries(bbox.get("require")) {
        let name = entry.get(0).and_then(Value::as_str).unwrap_or_default();
        match name {
            "ScheduledServerJS" | "ScheduledServerJSWithCSS" => {
                if entry.get(1).and_then(Value::as_str) != Some("handle") {
                    continue;
                }
                for inner in entries(entry.get(3)) {
                    walk_inner(inner, configs, versions);
                }
            }
            name if name.starts_with("CometPlatformRootClient") => {
                if entry.get(1).and_then(Value::as_str) == Some("init") {
                    collect_versions(entry.get(3), versions);
                }
            }
            _ => {}
        }
    }
    for entry in entries(bbox.get("define")) {
        store_config(entry, configs);
    }
}

fn walk_inner(value: &Value, configs: &mut BTreeMap<String, Value>, versions: &mut Vec<i64>) {
    if value.is_array() {
        store_config(value, configs);
    } else {
        walk_bbox(value, configs, versions);
    }
}

/// A `define` entry is `[name, unused, config, config_id]`; only positive config ids are real.
fn store_config(entry: &Value, configs: &mut BTreeMap<String, Value>) {
    let Some(items) = entry.as_array() else {
        return;
    };
    let (Some(name), Some(config), Some(id)) = (items.first(), items.get(2), items.get(3)) else {
        return;
    };
    let (Some(name), Some(id)) = (name.as_str(), id.as_i64()) else {
        return;
    };
    if id > 0 {
        configs.insert(name.to_owned(), config.clone());
    }
}

/// The expected preloaders of the root client carry the lightspeed request payload and its version.
fn collect_versions(inner: Option<&Value>, versions: &mut Vec<i64>) {
    let Some(preloaders) = inner
        .and_then(Value::as_array)
        .and_then(|items| items.get(4))
        .and_then(Value::as_array)
    else {
        return;
    };
    for preloader in preloaders {
        let id = preloader
            .get("preloaderID")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !id.starts_with("adp_LSPlatformGraphQLLightspeedRequest") {
            continue;
        }
        let payload = match preloader.get("variables") {
            Some(Value::String(text)) => serde_json::from_str::<Value>(text).ok(),
            Some(object) => Some(object.clone()),
            None => None,
        };
        let request = payload
            .as_ref()
            .and_then(|vars| vars.get("requestPayload"))
            .and_then(Value::as_str)
            .and_then(|text| serde_json::from_str::<Value>(text).ok());
        if let Some(version) = request
            .as_ref()
            .and_then(|request| request.get("version"))
            .and_then(Value::as_i64)
        {
            versions.push(version);
        }
    }
}

/// The entries of a `require` or `define` list, or of a nested list.
fn entries(list: Option<&Value>) -> Vec<&Value> {
    match list.and_then(Value::as_array) {
        Some(items) => items.iter().collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(scripts: &[&str]) -> String {
        let mut html = String::from("<html><head>");
        for script in scripts {
            html.push_str("<script type=\"application/json\" data-x=\"1\">");
            html.push_str(script);
            html.push_str("</script>");
        }
        html.push_str("</head><body></body></html>");
        html
    }

    const SCRIPT: &str = r#"{"__bbox":{"define":[
        ["CurrentUserInitialData",null,{"ACCOUNT_ID":"100","APP_ID":"2220391788200892","NAME":"Ada","USER_ID":"100"},1],
        ["MqttWebDeviceID",null,{"clientID":"device-1"},2],
        ["DGWWebConfig",null,{"appId":"772021112871879"},3],
        ["LSPlatformMessengerSyncParams",null,{"mailbox":"{\"mb\":1}","contact":"","e2ee":""},4],
        ["Ignored",null,{"x":1},0]
    ]}}"#;

    #[test]
    fn the_defined_configs_become_the_bootstrap_values() {
        let bootstrap = parse(&page(&[SCRIPT])).expect("parses");
        assert_eq!(bootstrap.user_id, "100");
        assert_eq!(bootstrap.name, "Ada");
        assert_eq!(bootstrap.app_id, "2220391788200892");
        assert_eq!(bootstrap.socket_app_id, "772021112871879");
        assert_eq!(bootstrap.device_id, "device-1");
        assert_eq!(bootstrap.mailbox_sync_params, "{\"mb\":1}");
        assert_eq!(bootstrap.version_id, 0);
    }

    #[test]
    fn configs_inside_a_scheduled_server_js_wrapper_are_read() {
        let wrapped = r#"{"__bbox":{"require":[
            ["ScheduledServerJS","handle",[],[
                {"__bbox":{"define":[["MqttWebDeviceID",null,{"clientID":"nested"},5]]}}
            ]]
        ]}}"#;
        let bootstrap = parse(&page(&[SCRIPT, wrapped])).expect("parses");
        // Definitions are stored in page order, so the later one replaces the earlier.
        assert_eq!(bootstrap.device_id, "nested");
    }

    #[test]
    fn the_lightspeed_version_comes_from_the_root_client_preloaders() {
        let root = r#"{"__bbox":{"require":[
            ["CometPlatformRootClient","init",[],[null,null,null,null,[
                {"preloaderID":"adp_LSPlatformGraphQLLightspeedRequestQuery_1",
                 "variables":"{\"requestPayload\":\"{\\\"version\\\":234}\"}"},
                {"preloaderID":"adp_Other_1","variables":"{}"}
            ]]]
        ]}}"#;
        let bootstrap = parse(&page(&[SCRIPT, root])).expect("parses");
        assert_eq!(bootstrap.version_id, 234);
    }

    #[test]
    fn a_page_with_no_user_is_an_auth_failure() {
        let html = page(&[r#"{"USER_ID":"0"}"#]).replace("<body>", "\"USER_ID\":\"0\"<body>");
        assert!(matches!(parse(&html), Err(Error::Auth(_))));
    }

    #[test]
    fn a_page_without_the_user_record_is_a_protocol_error() {
        assert!(matches!(parse(&page(&["{}"])), Err(Error::Protocol(_))));
        assert!(matches!(parse("<html>no scripts</html>"), Err(Error::Protocol(_))));
    }
}
