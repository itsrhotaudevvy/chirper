//! The network registry: what this build can connect to, and what it cannot yet.

use std::sync::Arc;

use chirper_bridge::Network;

use crate::model::{Capabilities, Network as WireNetwork};

/// The networks this build can actually use.
pub fn available(demo: bool) -> Vec<Arc<dyn Network>> {
    #[allow(unused_mut)]
    let mut out: Vec<Arc<dyn Network>> = Vec::new();
    out.push(Arc::new(chirper_net_discord::Discord::new()));
    if demo {
        out.push(Arc::new(chirper_net_testnet::TestNet::new()));
    }
    out
}

/// Networks the app lists but cannot connect to yet. They keep their place in the UI and explain
/// themselves instead of silently disappearing.
const UNPORTED: &[(&str, &str, &str)] = &[
    ("fluxer", "Fluxer", "#FF5500"),
    ("telegram", "Telegram", "#2AABEE"),
    ("signal", "Signal", "#3A76F0"),
    ("whatsapp", "WhatsApp", "#25D366"),
    ("facebook", "Facebook", "#1877F2"),
    ("instagram", "Instagram", "#E1306C"),
    ("imessage", "iMessage", "#34C759"),
    ("sms", "SMS", "#8E8E93"),
];

/// Every network of this build: the usable ones first, then the rest with their reason.
pub fn all(demo: bool) -> Vec<WireNetwork> {
    let mut out = available(demo)
        .iter()
        .map(|driver| super::convert::wire_network(driver.as_ref()))
        .collect::<Vec<_>>();
    for (id, name, color) in UNPORTED {
        out.push(WireNetwork {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            color: (*color).to_owned(),
            beta: false,
            available: false,
            reason: format!(
                "{name} has not been ported to the unified Rust runtime yet. \
                 The driver lands once its protocol crate is ready."
            ),
            login_flows: Vec::new(),
            capabilities: Capabilities::default(),
        });
    }
    out
}