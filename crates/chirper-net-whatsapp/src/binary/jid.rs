//! WhatsApp JIDs: `user@server`, with the device and agent that multi-device adds.
//!
//! Text form: `user[.agent][:device]@server`. The device is the companion's number (`:5`); the agent
//! is a legacy dotted field kept on non-phone servers. Phone (`s.whatsapp.net`) and LID user parts
//! may contain dots, so only the other servers split an agent off.

use std::fmt;
use std::str::FromStr;

use chirper_bridge::Error;

/// The servers a JID can belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Server {
    /// `s.whatsapp.net`: phone-number accounts.
    Pn,
    /// `lid`: the privacy identifiers that replace phone numbers.
    Lid,
    /// `g.us`: groups.
    Group,
    /// `broadcast`: status and broadcast lists.
    Broadcast,
    /// `newsletter`: channels.
    Newsletter,
    /// `hosted`: business-hosted accounts.
    Hosted,
    /// `hosted.lid`: business-hosted LID accounts.
    HostedLid,
    /// `msgr`: Messenger accounts.
    Messenger,
    /// `interop`: third-party interop accounts.
    Interop,
    /// `bot`: AI and business bots.
    Bot,
    /// `c.us`: the legacy phone-number server.
    Legacy,
    /// `call`: call signalling.
    Call,
}

impl Server {
    /// The server's name as it appears after the `@`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pn => "s.whatsapp.net",
            Self::Lid => "lid",
            Self::Group => "g.us",
            Self::Broadcast => "broadcast",
            Self::Newsletter => "newsletter",
            Self::Hosted => "hosted",
            Self::HostedLid => "hosted.lid",
            Self::Messenger => "msgr",
            Self::Interop => "interop",
            Self::Bot => "bot",
            Self::Legacy => "c.us",
            Self::Call => "call",
        }
    }

    /// The server a name refers to, if it is one this client knows.
    pub fn parse_known(name: &str) -> Option<Self> {
        Some(match name {
            "s.whatsapp.net" => Self::Pn,
            "lid" => Self::Lid,
            "g.us" => Self::Group,
            "broadcast" => Self::Broadcast,
            "newsletter" => Self::Newsletter,
            "hosted" => Self::Hosted,
            "hosted.lid" => Self::HostedLid,
            "msgr" => Self::Messenger,
            "interop" => Self::Interop,
            "bot" => Self::Bot,
            "c.us" => Self::Legacy,
            "call" => Self::Call,
            _ => return None,
        })
    }

    /// Whether the AD (agent/device) binary form can carry this server: its domain type must
    /// round-trip back to it.
    pub fn supports_ad(self) -> bool {
        matches!(self, Self::Pn | Self::Lid | Self::Hosted | Self::HostedLid)
    }

    /// The domain type byte an AD JID carries for this server.
    pub fn domain_type(self) -> u8 {
        match self {
            Self::Lid => 1,
            Self::Hosted => 128,
            Self::HostedLid => 129,
            _ => 0,
        }
    }

    /// The server an AD domain-type byte names. Even values with the high bit set are hosted, as
    /// WhatsApp's own decoder reads them.
    pub fn from_domain_type(domain: u8) -> Option<Self> {
        match domain {
            0 => Some(Self::Pn),
            1 => Some(Self::Lid),
            128 => Some(Self::Hosted),
            129 => Some(Self::HostedLid),
            n if n & 128 != 0 && n & 1 == 0 => Some(Self::Hosted),
            _ => None,
        }
    }
}

/// A JID: a user on a server, optionally on one of its devices.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Jid {
    pub user: String,
    pub server: Server,
    /// The companion device number; 0 for the primary device or no device.
    pub device: u16,
    /// The legacy agent number, only ever non-zero on servers without AD form.
    pub agent: u8,
    /// The interop integrator id; 0 when there is none.
    pub integrator: u16,
}

impl Jid {
    /// A JID with no device, agent or integrator.
    pub fn new(user: impl Into<String>, server: Server) -> Self {
        Self {
            user: user.into(),
            server,
            device: 0,
            agent: 0,
            integrator: 0,
        }
    }

    /// The same JID on device `device`.
    pub fn with_device(mut self, device: u16) -> Self {
        self.device = device;
        self
    }
}

impl fmt::Display for Jid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.user)?;
        if self.agent > 0 {
            write!(f, ".{}", self.agent)?;
        }
        if self.device > 0 {
            write!(f, ":{}", self.device)?;
        }
        write!(f, "@{}", self.server.as_str())
    }
}

impl FromStr for Jid {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::Protocol("a JID is not user@server".to_owned());
        let (user_part, server_name) = text.split_once('@').ok_or_else(invalid)?;
        let server = Server::parse_known(server_name).ok_or_else(invalid)?;
        if user_part.is_empty() {
            return Err(invalid());
        }

        let (before_device, device) = match user_part.split_once(':') {
            Some((before, device)) => {
                let device = device.parse::<u16>().map_err(|_| invalid())?;
                (before, device)
            }
            None => (user_part, 0),
        };
        let (user, agent) = match server {
            Server::Pn | Server::Lid => (before_device, 0),
            _ => match before_device.rsplit_once('.') {
                Some((user, agent)) => match agent.parse::<u8>() {
                    Ok(agent) => (user, agent),
                    Err(_) => (before_device, 0),
                },
                None => (before_device, 0),
            },
        };
        if user.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            user: user.to_owned(),
            server,
            device,
            agent,
            integrator: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_common_shapes() {
        let phone: Jid = "15551234567@s.whatsapp.net".parse().expect("parses");
        assert_eq!(phone, Jid::new("15551234567", Server::Pn));

        let device: Jid = "15551234567:5@s.whatsapp.net".parse().expect("parses");
        assert_eq!(device.device, 5);
        assert_eq!(device.to_string(), "15551234567:5@s.whatsapp.net");

        let group: Jid = "120363000000000000@g.us".parse().expect("parses");
        assert_eq!(group.server, Server::Group);
        assert_eq!(group.to_string(), "120363000000000000@g.us");
    }

    #[test]
    fn agent_is_split_only_on_non_phone_servers() {
        let bot: Jid = "1234.7:2@bot".parse().expect("parses");
        assert_eq!((bot.user.as_str(), bot.agent, bot.device), ("1234", 7, 2));
        assert_eq!(bot.to_string(), "1234.7:2@bot");

        let lid: Jid = "abc.def:3@lid".parse().expect("parses");
        assert_eq!(lid.user, "abc.def");
        assert_eq!(lid.agent, 0);
        assert_eq!(lid.device, 3);
    }

    #[test]
    fn rejects_malformed_jids() {
        for bad in [
            "",
            "@s.whatsapp.net",
            "nope",
            "1@unknown.server",
            "1:x@s.whatsapp.net",
            "1:70000@s.whatsapp.net",
        ] {
            assert!(bad.parse::<Jid>().is_err(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn domain_types_round_trip() {
        for server in [Server::Pn, Server::Lid, Server::Hosted, Server::HostedLid] {
            assert!(server.supports_ad());
            assert_eq!(Server::from_domain_type(server.domain_type()), Some(server));
        }
        assert!(!Server::Group.supports_ad());
        assert_eq!(Server::from_domain_type(2), None);
        assert_eq!(Server::from_domain_type(130), Some(Server::Hosted));
    }

    #[test]
    fn every_server_name_round_trips() {
        for server in [
            Server::Pn,
            Server::Lid,
            Server::Group,
            Server::Broadcast,
            Server::Newsletter,
            Server::Hosted,
            Server::HostedLid,
            Server::Messenger,
            Server::Interop,
            Server::Bot,
            Server::Legacy,
            Server::Call,
        ] {
            assert_eq!(Server::parse_known(server.as_str()), Some(server));
        }
    }
}
