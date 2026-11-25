//! The network registry: what this build can connect to, and what it cannot yet.

use std::sync::Arc;

use chirper_bridge::Network;

use crate::model::{Capabilities, Network as WireNetwork};

/// Drivers the host application provides, because they need the platform (the Android SMS
/// provider, for one). A host driver replaces the built-in entry with the same id.
#[derive(Clone, Default)]
pub struct HostNetworks(pub Vec<Arc<dyn Network>>);

impl std::fmt::Debug for HostNetworks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|driver| driver.id()))
            .finish()
    }
}

/// Every driver of this build, the host's first. A driver may still report itself unavailable
/// (see [`Network::unavailable`]); it is listed with its reason then.
pub fn available(demo: bool, host: &HostNetworks) -> Vec<Arc<dyn Network>> {
    let mut out: Vec<Arc<dyn Network>> = host.0.clone();
    let mut push = |driver: Arc<dyn Network>| {
        if !out.iter().any(|known| known.id() == driver.id()) {
            out.push(driver);
        }
    };
    push(Arc::new(chirper_net_whatsapp::WhatsApp::new()));
    push(Arc::new(chirper_net_telegram::Telegram::new()));
    push(Arc::new(chirper_net_discord::Discord::new()));
    push(Arc::new(chirper_net_fluxer::FluxerNet::new()));
    push(Arc::new(chirper_net_sms::Sms::new()));
    #[cfg(feature = "net-signal")]
    push(Arc::new(chirper_net_signal::Signal::new()));
    #[cfg(feature = "net-meta")]
    {
        push(Arc::new(chirper_net_meta::Messenger::new()));
        push(Arc::new(chirper_net_meta::Instagram::new()));
    }
    #[cfg(feature = "net-twitter")]
    push(Arc::new(chirper_net_twitter::Twitter::new()));
    #[cfg(feature = "net-linkedin")]
    push(Arc::new(chirper_net_linkedin::LinkedIn::new()));
    #[cfg(feature = "net-gmessages")]
    push(Arc::new(chirper_net_gmessages::GMessages::new()));
    #[cfg(feature = "net-gvoice")]
    push(Arc::new(chirper_net_gvoice::GVoice::new()));
    #[cfg(feature = "net-slack")]
    push(Arc::new(chirper_net_slack::Slack::new()));
    #[cfg(feature = "net-bluesky")]
    push(Arc::new(chirper_net_bluesky::Bluesky::new()));
    if demo {
        push(Arc::new(chirper_net_testnet::TestNet::new()));
    }
    out
}

/// Networks the app lists but has no driver for. They keep their place in the UI and explain
/// themselves instead of silently disappearing. Not shown in the default build.
#[cfg(feature = "more-networks")]
const UNPORTED: &[(&str, &str, &str)] = &[("imessage", "iMessage", "#34C759")];
#[cfg(not(feature = "more-networks"))]
const UNPORTED: &[(&str, &str, &str)] = &[];

/// Every network of this build: the drivers first, usable ones before the others, then the
/// networks without a driver, each with its reason.
pub fn all(drivers: &[Arc<dyn Network>]) -> Vec<WireNetwork> {
    let mut out = drivers
        .iter()
        .map(|driver| super::convert::wire_network(driver.as_ref()))
        .collect::<Vec<_>>();
    // Stable: usable networks first, each group in registry order.
    out.sort_by_key(|network| !network.available);
    for (id, name, color) in UNPORTED {
        if out.iter().any(|network| network.id == *id) {
            continue;
        }
        out.push(WireNetwork {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            color: (*color).to_owned(),
            beta: false,
            available: false,
            reason: format!(
                "{name} has not been ported to the unified Rust runtime yet. \
                 The driver lands once its protocol client is ready."
            ),
            login_flows: Vec::new(),
            capabilities: Capabilities::default(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_network_is_listed_once() {
        let drivers = available(true, &HostNetworks::default());
        let listed = all(&drivers);
        let mut ids = listed.iter().map(|n| n.id.as_str()).collect::<Vec<_>>();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "a network is listed twice");
        for id in [
            "whatsapp", "telegram", "discord", "fluxer", "sms", "testnet",
        ] {
            assert!(ids.contains(&id), "{id} is missing");
        }
        #[cfg(feature = "more-networks")]
        for id in [
            "signal",
            "messenger",
            "instagram",
            "twitter",
            "linkedin",
            "gmessages",
            "gvoice",
            "slack",
            "bluesky",
        ] {
            assert!(ids.contains(&id), "{id} is missing");
        }
        #[cfg(not(feature = "more-networks"))]
        assert_eq!(ids.len(), 6, "the default build lists exactly six networks");
    }

    #[test]
    fn usable_networks_come_first() {
        let drivers = available(true, &HostNetworks::default());
        let listed = all(&drivers);
        let first_unusable = listed
            .iter()
            .position(|n| !n.available)
            .unwrap_or(listed.len());
        assert!(listed[first_unusable..].iter().all(|n| !n.available));
    }
}
