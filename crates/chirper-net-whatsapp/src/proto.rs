//! The WhatsApp protobuf messages the handshake, pairing and login use, hand-written as `prost`
//! derives.
//!
//! Field numbers and wire types are copied from the `whatsapp.proto` schema of `waproto` 0.7.0
//! (MIT), https://github.com/jlucaso1/whatsapp-rust. Only the messages this phase touches are
//! here, and within them only the fields this client reads or writes. Fields left out are
//! unknown to us and skipped by `prost` on decode, so the server may send more than we model.
//!
//! Proto2 enums are carried as `int32` values with the constants next to their message, which is
//! the same varint on the wire and keeps an unknown enum value decodable.

// Protocol data from waproto 0.7.0 (MIT), https://github.com/jlucaso1/whatsapp-rust

pub mod e2e;
pub mod history;
pub mod sync;

/// `waCommon.MessageKey`: names one message of a chat. Field numbers from whatsmeow's
/// `proto/waCommon/WACommon.proto` (Mozilla Public License 2.0).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MessageKey {
    #[prost(string, optional, tag = "1")]
    pub remote_jid: Option<String>,
    #[prost(bool, optional, tag = "2")]
    pub from_me: Option<bool>,
    #[prost(string, optional, tag = "3")]
    pub id: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub participant: Option<String>,
}

/// `ADVEncryptionType` of the account and device identities.
pub mod adv_encryption_type {
    pub const E2EE: i32 = 0;
    pub const HOSTED: i32 = 1;
    pub const NON_E2EE: i32 = 2;
}

/// `ADVDeviceIdentity`: the device's own identity details, signed by the account.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AdvDeviceIdentity {
    #[prost(uint32, optional, tag = "1")]
    pub raw_id: Option<u32>,
    #[prost(uint64, optional, tag = "2")]
    pub timestamp: Option<u64>,
    #[prost(uint32, optional, tag = "3")]
    pub key_index: Option<u32>,
    #[prost(int32, optional, tag = "4")]
    pub account_type: Option<i32>,
    #[prost(int32, optional, tag = "5")]
    pub device_type: Option<i32>,
}

/// `ADVKeyIndexList`: the key indexes the account currently accepts for devices.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AdvKeyIndexList {
    #[prost(uint32, optional, tag = "1")]
    pub raw_id: Option<u32>,
    #[prost(uint64, optional, tag = "2")]
    pub timestamp: Option<u64>,
    #[prost(uint32, optional, tag = "3")]
    pub current_index: Option<u32>,
    #[prost(uint32, repeated, packed = "true", tag = "4")]
    pub valid_indexes: Vec<u32>,
    #[prost(int32, optional, tag = "5")]
    pub account_type: Option<i32>,
}

/// `ADVSignedDeviceIdentity`: the details plus the account and device signatures.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AdvSignedDeviceIdentity {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub details: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub account_signature_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub account_signature: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub device_signature: Option<Vec<u8>>,
}

/// `ADVSignedDeviceIdentityHMAC`: the container the primary sends in `pair-success`, authenticated
/// with the ADV secret shared through the QR code.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AdvSignedDeviceIdentityHmac {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub details: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub hmac: Option<Vec<u8>>,
    #[prost(int32, optional, tag = "3")]
    pub account_type: Option<i32>,
}

/// `ADVSignedKeyIndexList`: the signed key index list.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AdvSignedKeyIndexList {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub details: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub account_signature: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub account_signature_key: Option<Vec<u8>>,
}

/// `CertChain`: the server's two-step Noise certificate chain.
#[derive(Clone, PartialEq, prost::Message)]
pub struct CertChain {
    #[prost(message, optional, tag = "1")]
    pub leaf: Option<cert_chain::NoiseCertificate>,
    #[prost(message, optional, tag = "2")]
    pub intermediate: Option<cert_chain::NoiseCertificate>,
}

/// Nested messages of [`CertChain`].
pub mod cert_chain {
    /// `CertChain.NoiseCertificate`: signed details plus the XEdDSA signature over them.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct NoiseCertificate {
        #[prost(bytes = "vec", optional, tag = "1")]
        pub details: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "2")]
        pub signature: Option<Vec<u8>>,
    }

    /// `CertChain.NoiseCertificate.Details`.
    pub mod noise_certificate {
        #[derive(Clone, PartialEq, prost::Message)]
        pub struct Details {
            #[prost(uint32, optional, tag = "1")]
            pub serial: Option<u32>,
            #[prost(uint32, optional, tag = "2")]
            pub issuer_serial: Option<u32>,
            #[prost(bytes = "vec", optional, tag = "3")]
            pub key: Option<Vec<u8>>,
            #[prost(uint64, optional, tag = "4")]
            pub not_before: Option<u64>,
            #[prost(uint64, optional, tag = "5")]
            pub not_after: Option<u64>,
        }
    }
}

/// `ClientPayload`: what the client says about itself after the handshake, either to register a
/// new device or to log a stored one in.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientPayload {
    #[prost(uint64, optional, tag = "1")]
    pub username: Option<u64>,
    #[prost(bool, optional, tag = "3")]
    pub passive: Option<bool>,
    #[prost(message, optional, tag = "5")]
    pub user_agent: Option<client_payload::UserAgent>,
    #[prost(message, optional, tag = "6")]
    pub web_info: Option<client_payload::WebInfo>,
    #[prost(string, optional, tag = "7")]
    pub push_name: Option<String>,
    #[prost(int32, optional, tag = "12")]
    pub connect_type: Option<i32>,
    #[prost(int32, optional, tag = "13")]
    pub connect_reason: Option<i32>,
    #[prost(uint32, optional, tag = "16")]
    pub connect_attempt_count: Option<u32>,
    #[prost(uint32, optional, tag = "18")]
    pub device: Option<u32>,
    #[prost(message, optional, tag = "19")]
    pub device_pairing_data: Option<client_payload::DevicePairingRegistrationData>,
    #[prost(int32, optional, tag = "24")]
    pub lc: Option<i32>,
    #[prost(bool, optional, tag = "33")]
    pub pull: Option<bool>,
    #[prost(bool, optional, tag = "41")]
    pub lid_db_migrated: Option<bool>,
}

/// Values of `ClientPayload.ConnectType` and `ConnectReason` the client sends.
pub mod client_payload {
    /// `ClientPayload.ConnectType`.
    pub mod connect_type {
        pub const WIFI_UNKNOWN: i32 = 1;
    }

    /// `ClientPayload.ConnectReason`.
    pub mod connect_reason {
        pub const USER_ACTIVATED: i32 = 1;
    }

    /// `ClientPayload.UserAgent`: the platform, version and locale the client presents.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct UserAgent {
        #[prost(int32, optional, tag = "1")]
        pub platform: Option<i32>,
        #[prost(message, optional, tag = "2")]
        pub app_version: Option<user_agent::AppVersion>,
        #[prost(string, optional, tag = "3")]
        pub mcc: Option<String>,
        #[prost(string, optional, tag = "4")]
        pub mnc: Option<String>,
        #[prost(string, optional, tag = "5")]
        pub os_version: Option<String>,
        #[prost(string, optional, tag = "6")]
        pub manufacturer: Option<String>,
        #[prost(string, optional, tag = "7")]
        pub device: Option<String>,
        #[prost(string, optional, tag = "8")]
        pub os_build_number: Option<String>,
        #[prost(string, optional, tag = "9")]
        pub phone_id: Option<String>,
        #[prost(int32, optional, tag = "10")]
        pub release_channel: Option<i32>,
        #[prost(string, optional, tag = "11")]
        pub locale_language_iso6391: Option<String>,
        #[prost(string, optional, tag = "12")]
        pub locale_country_iso31661_alpha2: Option<String>,
    }

    /// Values of `UserAgent.Platform` and `ReleaseChannel`.
    pub mod user_agent {
        /// `UserAgent.Platform`.
        pub mod platform {
            pub const WEB: i32 = 14;
        }

        /// `UserAgent.ReleaseChannel`.
        pub mod release_channel {
            pub const RELEASE: i32 = 0;
        }

        /// `UserAgent.AppVersion`: the WhatsApp build the client claims to be.
        #[derive(Clone, PartialEq, prost::Message)]
        pub struct AppVersion {
            #[prost(uint32, optional, tag = "1")]
            pub primary: Option<u32>,
            #[prost(uint32, optional, tag = "2")]
            pub secondary: Option<u32>,
            #[prost(uint32, optional, tag = "3")]
            pub tertiary: Option<u32>,
            #[prost(uint32, optional, tag = "4")]
            pub quaternary: Option<u32>,
            #[prost(uint32, optional, tag = "5")]
            pub quinary: Option<u32>,
        }
    }

    /// `ClientPayload.WebInfo`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct WebInfo {
        #[prost(int32, optional, tag = "4")]
        pub web_sub_platform: Option<i32>,
    }

    /// `ClientPayload.DevicePairingRegistrationData`: the keys a new device registers with.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct DevicePairingRegistrationData {
        #[prost(bytes = "vec", optional, tag = "1")]
        pub e_regid: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "2")]
        pub e_keytype: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "3")]
        pub e_ident: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "4")]
        pub e_skey_id: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "5")]
        pub e_skey_val: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "6")]
        pub e_skey_sig: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "7")]
        pub build_hash: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "8")]
        pub device_props: Option<Vec<u8>>,
    }
}

/// `DeviceProps`: the companion's self-description, sent inside registration.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DeviceProps {
    #[prost(string, optional, tag = "1")]
    pub os: Option<String>,
    #[prost(message, optional, tag = "2")]
    pub version: Option<device_props::AppVersion>,
    #[prost(int32, optional, tag = "3")]
    pub platform_type: Option<i32>,
    #[prost(bool, optional, tag = "4")]
    pub require_full_sync: Option<bool>,
}

/// Nested messages and values of [`DeviceProps`].
pub mod device_props {
    /// `DeviceProps.PlatformType`: the kind of browser or device, used to pick the linked-device
    /// label on the phone.
    pub mod platform_type {
        pub const UNKNOWN: i32 = 0;
        pub const CHROME: i32 = 1;
        pub const FIREFOX: i32 = 2;
        pub const SAFARI: i32 = 5;
        pub const EDGE: i32 = 6;
        pub const DESKTOP: i32 = 7;
    }

    /// `DeviceProps.AppVersion`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct AppVersion {
        #[prost(uint32, optional, tag = "1")]
        pub primary: Option<u32>,
        #[prost(uint32, optional, tag = "2")]
        pub secondary: Option<u32>,
        #[prost(uint32, optional, tag = "3")]
        pub tertiary: Option<u32>,
        #[prost(uint32, optional, tag = "4")]
        pub quaternary: Option<u32>,
        #[prost(uint32, optional, tag = "5")]
        pub quinary: Option<u32>,
    }
}

/// `HandshakeMessage`: the three Noise XX messages, each wrapped in one of its branches.
#[derive(Clone, PartialEq, prost::Message)]
pub struct HandshakeMessage {
    #[prost(message, optional, tag = "2")]
    pub client_hello: Option<handshake_message::ClientHello>,
    #[prost(message, optional, tag = "3")]
    pub server_hello: Option<handshake_message::ServerHello>,
    #[prost(message, optional, tag = "4")]
    pub client_finish: Option<handshake_message::ClientFinish>,
}

/// Nested messages of [`HandshakeMessage`].
pub mod handshake_message {
    /// `HandshakeMessage.ClientHello`: the client's ephemeral key.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ClientHello {
        #[prost(bytes = "vec", optional, tag = "1")]
        pub ephemeral: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "2")]
        pub r#static: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "3")]
        pub payload: Option<Vec<u8>>,
    }

    /// `HandshakeMessage.ServerHello`: the server's ephemeral, encrypted static and certificate.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ServerHello {
        #[prost(bytes = "vec", optional, tag = "1")]
        pub ephemeral: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "2")]
        pub r#static: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "3")]
        pub payload: Option<Vec<u8>>,
    }

    /// `HandshakeMessage.ClientFinish`: the client's encrypted static and client payload.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ClientFinish {
        #[prost(bytes = "vec", optional, tag = "1")]
        pub r#static: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "2")]
        pub payload: Option<Vec<u8>>,
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::*;

    fn round_trip<M: Message + Default + PartialEq + std::fmt::Debug>(message: &M) {
        let bytes = message.encode_to_vec();
        let decoded = M::decode(bytes.as_slice()).expect("decodes what it encoded");
        assert_eq!(&decoded, message);
    }

    #[test]
    fn handshake_message_round_trips() {
        round_trip(&HandshakeMessage {
            client_hello: Some(handshake_message::ClientHello {
                ephemeral: Some(vec![7; 32]),
                ..Default::default()
            }),
            server_hello: Some(handshake_message::ServerHello {
                ephemeral: Some(vec![1; 32]),
                r#static: Some(vec![2; 48]),
                payload: Some(vec![3; 100]),
            }),
            client_finish: Some(handshake_message::ClientFinish {
                r#static: Some(vec![4; 48]),
                payload: Some(vec![5; 200]),
            }),
        });
    }

    #[test]
    fn cert_chain_round_trips() {
        round_trip(&CertChain {
            leaf: Some(cert_chain::NoiseCertificate {
                details: Some(vec![1, 2, 3]),
                signature: Some(vec![4; 64]),
            }),
            intermediate: Some(cert_chain::NoiseCertificate {
                details: Some(vec![5]),
                signature: None,
            }),
        });
        round_trip(&cert_chain::noise_certificate::Details {
            serial: Some(2),
            issuer_serial: Some(1),
            key: Some(vec![9; 32]),
            not_before: Some(1_700_000_000),
            not_after: Some(1_900_000_000),
        });
    }

    #[test]
    fn client_payload_round_trips() {
        round_trip(&ClientPayload {
            passive: Some(false),
            user_agent: Some(client_payload::UserAgent {
                platform: Some(client_payload::user_agent::platform::WEB),
                app_version: Some(client_payload::user_agent::AppVersion {
                    primary: Some(2),
                    secondary: Some(3000),
                    tertiary: Some(1_042_742_319),
                    ..Default::default()
                }),
                release_channel: Some(client_payload::user_agent::release_channel::RELEASE),
                ..Default::default()
            }),
            web_info: Some(client_payload::WebInfo {
                web_sub_platform: Some(0),
            }),
            connect_type: Some(client_payload::connect_type::WIFI_UNKNOWN),
            connect_reason: Some(client_payload::connect_reason::USER_ACTIVATED),
            device_pairing_data: Some(client_payload::DevicePairingRegistrationData {
                e_regid: Some(vec![0, 0, 0, 1]),
                e_keytype: Some(vec![5]),
                e_ident: Some(vec![6; 32]),
                e_skey_id: Some(vec![0, 0, 1]),
                e_skey_val: Some(vec![8; 32]),
                e_skey_sig: Some(vec![9; 64]),
                build_hash: Some(vec![10; 16]),
                device_props: Some(vec![11]),
            }),
            ..Default::default()
        });
        round_trip(&ClientPayload {
            username: Some(15_551_234_567),
            device: Some(3),
            passive: Some(true),
            pull: Some(true),
            lc: Some(0),
            lid_db_migrated: Some(false),
            ..Default::default()
        });
    }

    #[test]
    fn device_props_round_trips() {
        round_trip(&DeviceProps {
            os: Some("Chirper".to_owned()),
            version: Some(device_props::AppVersion {
                primary: Some(0),
                secondary: Some(1),
                ..Default::default()
            }),
            platform_type: Some(device_props::platform_type::CHROME),
            require_full_sync: Some(false),
        });
    }

    #[test]
    fn adv_messages_round_trip() {
        round_trip(&AdvDeviceIdentity {
            raw_id: Some(1),
            timestamp: Some(1_700_000_000),
            key_index: Some(7),
            account_type: Some(adv_encryption_type::E2EE),
            device_type: Some(adv_encryption_type::E2EE),
        });
        round_trip(&AdvKeyIndexList {
            raw_id: Some(1),
            timestamp: Some(2),
            current_index: Some(3),
            valid_indexes: vec![1, 2, 3, 300],
            account_type: Some(adv_encryption_type::HOSTED),
        });
        round_trip(&AdvSignedDeviceIdentity {
            details: Some(vec![1; 10]),
            account_signature_key: Some(vec![2; 32]),
            account_signature: Some(vec![3; 64]),
            device_signature: None,
        });
        round_trip(&AdvSignedDeviceIdentityHmac {
            details: Some(vec![4; 10]),
            hmac: Some(vec![5; 32]),
            account_type: Some(adv_encryption_type::E2EE),
        });
        round_trip(&AdvSignedKeyIndexList {
            details: Some(vec![6; 4]),
            account_signature: Some(vec![7; 64]),
            account_signature_key: Some(vec![8; 32]),
        });
    }

    #[test]
    fn unknown_fields_are_skipped() {
        // Field 999, varint wire type (key 0x1F38 as two bytes), value 1, after a `passive` field.
        let mut bytes = ClientPayload {
            passive: Some(true),
            ..Default::default()
        }
        .encode_to_vec();
        bytes.extend_from_slice(&[0xB8, 0x3E, 0x01]);
        let decoded = ClientPayload::decode(bytes.as_slice()).expect("unknown field is skipped");
        assert_eq!(decoded.passive, Some(true));
    }

    #[test]
    fn garbage_is_rejected_not_panicking() {
        assert!(HandshakeMessage::decode(&[0xFF, 0xFF, 0xFF][..]).is_err());
        assert!(CertChain::decode(&[0x0A, 0x20, 0x01][..]).is_err());
    }
}
