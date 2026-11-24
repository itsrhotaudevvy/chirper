//! Protobuf messages of the pairing, authentication, UKEY2 and RPC envelope schemas.
//!
//! Hand-written `prost` structs with the field numbers of `libgm/gmproto` (`authentication.proto`,
//! `rpc.proto`, `ukey.proto`, `events.proto`, `util.proto`). Enums are plain `int32` fields, as
//! protobuf encodes them on the wire; the values the driver uses are named in [`bugle_route`],
//! [`action`] and [`message_type`]. Port of the generated Go code, with the Go schema as the source.

/// `BugleRoute` values of `IncomingRPCMessage.bugleRoute`.
pub mod bugle_route {
    pub const DATA_EVENT: i32 = 19;
    pub const PAIR_EVENT: i32 = 14;
    pub const GAIA_EVENT: i32 = 7;
}

/// `MessageType` values of the RPC envelope.
pub mod message_type {
    pub const BUGLE_MESSAGE: i32 = 2;
    pub const GAIA_1: i32 = 3;
    pub const GAIA_2: i32 = 20;
}

/// `ActionType` values of `OutgoingRPCData.action` and `RPCMessageData.action`.
pub mod action {
    pub const LIST_CONVERSATIONS: i32 = 1;
    pub const LIST_MESSAGES: i32 = 2;
    pub const SEND_MESSAGE: i32 = 3;
    pub const MESSAGE_UPDATES: i32 = 4;
    pub const CONVERSATION_UPDATES: i32 = 7;
    pub const GET_OR_CREATE_CONVERSATION: i32 = 9;
    pub const MESSAGE_READ: i32 = 10;
    pub const TYPING_UPDATES: i32 = 12;
    pub const SETTINGS_UPDATE: i32 = 13;
    pub const USER_ALERT: i32 = 14;
    pub const GET_UPDATES: i32 = 16;
    pub const ACK_BROWSER_PRESENCE: i32 = 17;
    pub const NOTIFY_DITTO_ACTIVITY: i32 = 22;
    pub const DELETE_MESSAGE: i32 = 23;
    pub const GET_CONVERSATION_TYPE: i32 = 21;
    pub const GET_CONVERSATION: i32 = 43;
    pub const SEND_REACTION: i32 = 38;
    pub const GET_FULL_SIZE_IMAGE: i32 = 36;
    pub const GET_BLOB_FOR_ATTACHMENT: i32 = 40;
    pub const IS_BUGLE_DEFAULT: i32 = 31;
    pub const CREATE_GAIA_PAIRING: i32 = 42;
    pub const CREATE_GAIA_PAIRING_CLIENT_INIT: i32 = 44;
    pub const CREATE_GAIA_PAIRING_CLIENT_FINISHED: i32 = 45;
    pub const UNPAIR_GAIA_PAIRING: i32 = 46;
    pub const CANCEL_GAIA_PAIRING: i32 = 47;
}

/// `BrowserType.OTHER`.
pub const BROWSER_TYPE_OTHER: i32 = 1;
/// `DeviceType.TABLET`.
pub const DEVICE_TYPE_TABLET: i32 = 2;

/// `authentication.Device`: one phone or browser identity.
#[derive(Clone, PartialEq, prost::Message, serde::Serialize, serde::Deserialize)]
pub struct Device {
    #[prost(int64, tag = "1")]
    pub user_id: i64,
    #[prost(string, tag = "2")]
    pub source_id: String,
    #[prost(string, tag = "3")]
    pub network: String,
}

/// `authentication.BrowserDetails`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct BrowserDetails {
    #[prost(string, tag = "1")]
    pub user_agent: String,
    #[prost(int32, tag = "2")]
    pub browser_type: i32,
    #[prost(string, tag = "3")]
    pub os: String,
    #[prost(int32, tag = "6")]
    pub device_type: i32,
}

/// `authentication.ConfigVersion`: the web client version the server expects.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct ConfigVersion {
    #[prost(int32, tag = "3")]
    pub year: i32,
    #[prost(int32, tag = "4")]
    pub month: i32,
    #[prost(int32, tag = "5")]
    pub day: i32,
    #[prost(int32, tag = "7")]
    pub v1: i32,
    #[prost(int32, tag = "9")]
    pub v2: i32,
}

/// `authentication.AuthMessage`: the auth header of every request.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AuthMessage {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(string, tag = "3")]
    pub network: String,
    #[prost(bytes = "vec", tag = "6")]
    pub tachyon_auth_token: Vec<u8>,
    #[prost(message, optional, tag = "7")]
    pub config_version: Option<ConfigVersion>,
}

/// `authentication.TokenData`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TokenData {
    #[prost(bytes = "vec", tag = "1")]
    pub tachyon_auth_token: Vec<u8>,
    #[prost(int64, tag = "2")]
    pub ttl: i64,
}

/// `authentication.CoordinateMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct CoordinateMessage {
    #[prost(int64, tag = "2")]
    pub coord1: i64,
}

/// `authentication.RegisterPhoneRelayResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RegisterPhoneRelayResponse {
    #[prost(message, optional, tag = "1")]
    pub coordinates: Option<CoordinateMessage>,
    #[prost(message, optional, tag = "2")]
    pub browser: Option<Device>,
    #[prost(bytes = "vec", tag = "3")]
    pub pairing_key: Vec<u8>,
    #[prost(int64, tag = "4")]
    pub valid_for: i64,
    #[prost(message, optional, tag = "5")]
    pub auth_key_data: Option<TokenData>,
    #[prost(string, tag = "6")]
    pub response_id: String,
}

/// `authentication.RefreshPhoneRelayResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RefreshPhoneRelayResponse {
    #[prost(message, optional, tag = "1")]
    pub coordinates: Option<CoordinateMessage>,
    #[prost(bytes = "vec", tag = "2")]
    pub pair_key: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub valid_for: i64,
}

/// `authentication.WebEncryptionKeyResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct WebEncryptionKeyResponse {
    #[prost(message, optional, tag = "1")]
    pub coordinates: Option<CoordinateMessage>,
    #[prost(bytes = "vec", tag = "2")]
    pub key: Vec<u8>,
}

/// `authentication.ECDSAKeys`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct EcdsaKeys {
    #[prost(int64, tag = "1")]
    pub field1: i64,
    #[prost(bytes = "vec", tag = "2")]
    pub encrypted_keys: Vec<u8>,
}

/// `authentication.WebAuthKey`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct WebAuthKey {
    #[prost(bytes = "vec", tag = "1")]
    pub web_auth_key: Vec<u8>,
    #[prost(int64, tag = "2")]
    pub valid_for: i64,
}

/// `authentication.KeyData`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct KeyData {
    #[prost(message, optional, tag = "1")]
    pub mobile: Option<Device>,
    #[prost(message, optional, tag = "6")]
    pub ecdsa_keys: Option<EcdsaKeys>,
    #[prost(message, optional, tag = "2")]
    pub web_auth_key_data: Option<WebAuthKey>,
    #[prost(message, optional, tag = "3")]
    pub browser: Option<Device>,
}

/// `authentication.CurrentDeviceData`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct CurrentDeviceData {
    #[prost(message, optional, tag = "1")]
    pub browser: Option<Device>,
}

/// `authentication.AuthenticationContainer`: the body of `RegisterPhoneRelay`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AuthenticationContainer {
    #[prost(message, optional, tag = "1")]
    pub auth_message: Option<AuthMessage>,
    #[prost(message, optional, tag = "3")]
    pub browser_details: Option<BrowserDetails>,
    #[prost(oneof = "authentication_container::Data", tags = "4, 5")]
    pub data: Option<authentication_container::Data>,
}

/// Oneof and nested types of [`AuthenticationContainer`].
pub mod authentication_container {
    /// The `data` oneof.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Data {
        #[prost(message, tag = "4")]
        KeyData(super::KeyData),
        #[prost(message, tag = "5")]
        DeviceData(super::CurrentDeviceData),
    }
}

/// `authentication.RevokeRelayPairingResponse`: an object the driver does not read.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct RevokeRelayPairingResponse {}

/// `authentication.RevokeRelayPairingRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RevokeRelayPairingRequest {
    #[prost(message, optional, tag = "1")]
    pub auth_message: Option<AuthMessage>,
    #[prost(message, optional, tag = "2")]
    pub browser: Option<Device>,
}

/// `authentication.URLData`: the payload behind the QR code.
#[derive(Clone, PartialEq, prost::Message)]
pub struct UrlData {
    #[prost(bytes = "vec", tag = "1")]
    pub pairing_key: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub aes_key: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub hmac_key: Vec<u8>,
}

/// `authentication.PairedData`: the phone confirmed a QR pairing.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PairedData {
    #[prost(message, optional, tag = "1")]
    pub mobile: Option<Device>,
    #[prost(message, optional, tag = "2")]
    pub token_data: Option<TokenData>,
    #[prost(message, optional, tag = "3")]
    pub browser: Option<Device>,
}

/// `authentication.RevokePairData`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RevokePairData {
    #[prost(message, optional, tag = "1")]
    pub revoked_device: Option<Device>,
}

/// `authentication.ErrorResponse`: the body of a failed request.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ErrorResponse {
    #[prost(int64, tag = "1")]
    pub kind: i64,
    #[prost(string, tag = "2")]
    pub message: String,
}

/// `authentication.RegisterRefreshResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RegisterRefreshResponse {
    #[prost(message, optional, tag = "2")]
    pub token_data: Option<TokenData>,
}

/// `authentication.GaiaPairingRequestContainer`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GaiaPairingRequestContainer {
    #[prost(string, tag = "1")]
    pub pairing_attempt_id: String,
    #[prost(message, optional, tag = "2")]
    pub browser_details: Option<BrowserDetails>,
    #[prost(int64, tag = "3")]
    pub start_timestamp: i64,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,
    #[prost(int32, tag = "5")]
    pub proposed_verification_code_version: i32,
    #[prost(int32, tag = "6")]
    pub proposed_key_derivation_version: i32,
    #[prost(string, tag = "8")]
    pub private_api_confirmation: String,
}

/// `authentication.GaiaPairingResponseContainer`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GaiaPairingResponseContainer {
    #[prost(int32, tag = "1")]
    pub finish_error_type: i32,
    #[prost(int32, tag = "2")]
    pub finish_error_code: i32,
    #[prost(int32, tag = "3")]
    pub unknown_int3: i32,
    #[prost(string, tag = "4")]
    pub session_uuid: String,
    #[prost(bytes = "vec", tag = "5")]
    pub data: Vec<u8>,
    #[prost(int32, tag = "6")]
    pub confirmed_verification_code_version: i32,
    #[prost(int32, tag = "7")]
    pub confirmed_key_derivation_version: i32,
    #[prost(bytes = "vec", tag = "8")]
    pub unknown_bytes: Vec<u8>,
}

/// `authentication.RevokeGaiaPairingRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RevokeGaiaPairingRequest {
    #[prost(string, tag = "1")]
    pub pairing_attempt_id: String,
}

/// `authentication.SignInGaiaRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SignInGaiaRequest {
    #[prost(message, optional, tag = "1")]
    pub auth_message: Option<AuthMessage>,
    #[prost(message, optional, tag = "2")]
    pub inner: Option<sign_in_gaia_request::Inner>,
    #[prost(int32, tag = "3")]
    pub unknown_int3: i32,
    #[prost(string, tag = "4")]
    pub network: String,
}

/// Nested messages of [`SignInGaiaRequest`].
pub mod sign_in_gaia_request {
    /// `SignInGaiaRequest.Inner`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Inner {
        #[prost(message, optional, tag = "1")]
        pub device_id: Option<DeviceId>,
        #[prost(message, optional, tag = "36")]
        pub some_data: Option<Data>,
    }

    /// `SignInGaiaRequest.Inner.DeviceID`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct DeviceId {
        #[prost(int32, tag = "1")]
        pub unknown_int1: i32,
        #[prost(string, tag = "2")]
        pub device_id: String,
    }

    /// `SignInGaiaRequest.Inner.Data`: the refresh key's public part, sent base64 in pblite.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Data {
        #[prost(bytes = "vec", tag = "3")]
        pub some_data: Vec<u8>,
    }
}

/// `authentication.SignInGaiaResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SignInGaiaResponse {
    #[prost(message, optional, tag = "1")]
    pub header: Option<sign_in_gaia_response::Header>,
    #[prost(string, tag = "2")]
    pub maybe_browser_uuid: String,
    #[prost(message, optional, tag = "3")]
    pub device_data: Option<sign_in_gaia_response::DeviceData>,
    #[prost(message, optional, tag = "4")]
    pub token_data: Option<TokenData>,
}

/// Nested messages of [`SignInGaiaResponse`].
pub mod sign_in_gaia_response {
    use super::Device;
    use super::rpc_gaia_data::{Item2Item1, Item4};

    /// `SignInGaiaResponse.Header`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Header {
        #[prost(uint64, tag = "2")]
        pub unknown_int2: u64,
        #[prost(int64, tag = "4")]
        pub unknown_timestamp: i64,
    }

    /// `SignInGaiaResponse.DeviceData`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct DeviceData {
        #[prost(message, optional, tag = "1")]
        pub device_wrapper: Option<DeviceWrapper>,
        #[prost(message, repeated, tag = "2")]
        pub unknown_items2: Vec<Item2Item1>,
        #[prost(message, repeated, tag = "3")]
        pub unknown_items3: Vec<Item4>,
    }

    /// `SignInGaiaResponse.DeviceData.DeviceWrapper`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct DeviceWrapper {
        #[prost(message, optional, tag = "1")]
        pub device: Option<Device>,
    }
}

/// `authentication.Ukey2*`, `ukey.proto`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Ukey2Message {
    /// `Ukey2Message.Type`: 2 client init, 3 server init, 4 client finish.
    #[prost(int32, tag = "1")]
    pub message_type: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub message_data: Vec<u8>,
}

/// `Ukey2Message.Type` values.
pub mod ukey2_type {
    pub const CLIENT_INIT: i32 = 2;
    pub const SERVER_INIT: i32 = 3;
    pub const CLIENT_FINISH: i32 = 4;
}

/// `Ukey2ClientInit`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Ukey2ClientInit {
    #[prost(int32, tag = "1")]
    pub version: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub random: Vec<u8>,
    #[prost(message, repeated, tag = "3")]
    pub cipher_commitments: Vec<ukey2_client_init::CipherCommitment>,
    #[prost(string, tag = "4")]
    pub next_protocol: String,
}

/// Nested messages of [`Ukey2ClientInit`].
pub mod ukey2_client_init {
    /// `Ukey2ClientInit.CipherCommitment`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct CipherCommitment {
        #[prost(int32, tag = "1")]
        pub handshake_cipher: i32,
        #[prost(bytes = "vec", tag = "2")]
        pub commitment: Vec<u8>,
    }
}

/// `Ukey2HandshakeCipher::P256_SHA512`.
pub const UKEY2_P256_SHA512: i32 = 100;

/// `Ukey2ServerInit`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Ukey2ServerInit {
    #[prost(int32, tag = "1")]
    pub version: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub random: Vec<u8>,
    #[prost(int32, tag = "3")]
    pub handshake_cipher: i32,
    #[prost(message, optional, tag = "4")]
    pub public_key: Option<GenericPublicKey>,
}

/// `Ukey2ClientFinished`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Ukey2ClientFinished {
    #[prost(message, optional, tag = "1")]
    pub public_key: Option<GenericPublicKey>,
}

/// `GenericPublicKey` with its EC P-256 arm (the only one the driver uses).
#[derive(Clone, PartialEq, prost::Message)]
pub struct GenericPublicKey {
    /// `PublicKeyType::EC_P256`.
    #[prost(int32, tag = "1")]
    pub kind: i32,
    #[prost(message, optional, tag = "2")]
    pub ec_p256_public_key: Option<EcP256PublicKey>,
}

/// `EcP256PublicKey`: coordinates, possibly with a leading zero byte.
#[derive(Clone, PartialEq, prost::Message)]
pub struct EcP256PublicKey {
    #[prost(bytes = "vec", tag = "1")]
    pub x: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub y: Vec<u8>,
}

/// `rpc.StartAckMessage`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct StartAckMessage {
    #[prost(int32, optional, tag = "1")]
    pub count: Option<i32>,
}

/// `rpc.IncomingRPCMessage`: one event or response from the phone, as the long poll delivers it.
#[derive(Clone, PartialEq, prost::Message)]
pub struct IncomingRpcMessage {
    #[prost(string, tag = "1")]
    pub response_id: String,
    #[prost(int32, tag = "2")]
    pub bugle_route: i32,
    #[prost(uint64, tag = "3")]
    pub start_execute: u64,
    #[prost(int32, tag = "5")]
    pub message_type: i32,
    #[prost(uint64, tag = "6")]
    pub finish_execute: u64,
    #[prost(uint64, tag = "7")]
    pub microseconds_taken: u64,
    #[prost(message, optional, tag = "8")]
    pub mobile: Option<Device>,
    #[prost(message, optional, tag = "9")]
    pub browser: Option<Device>,
    /// An [`RpcMessageData`], [`RpcPairData`] or [`RpcGaiaData`] depending on the route.
    #[prost(bytes = "vec", tag = "12")]
    pub message_data: Vec<u8>,
    #[prost(string, tag = "17")]
    pub signature_id: String,
    #[prost(string, tag = "21")]
    pub timestamp: String,
    #[prost(message, optional, tag = "23")]
    pub gditto_source: Option<incoming_rpc_message::GdittoSource>,
}

/// Nested messages of [`IncomingRpcMessage`].
pub mod incoming_rpc_message {
    /// `IncomingRPCMessage.GDittoSource`.
    #[derive(Clone, Copy, PartialEq, prost::Message)]
    pub struct GdittoSource {
        #[prost(int32, tag = "2")]
        pub device_id: i32,
    }
}

/// `rpc.RPCMessageData`: the envelope of a data route message.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RpcMessageData {
    #[prost(string, tag = "1")]
    pub session_id: String,
    #[prost(int64, tag = "3")]
    pub timestamp: i64,
    #[prost(int32, tag = "4")]
    pub action: i32,
    #[prost(bytes = "vec", tag = "5")]
    pub unencrypted_data: Vec<u8>,
    #[prost(bool, tag = "6")]
    pub bool1: bool,
    #[prost(bool, tag = "7")]
    pub bool2: bool,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub encrypted_data: Option<Vec<u8>>,
    #[prost(bool, tag = "9")]
    pub bool3: bool,
    #[prost(bytes = "vec", optional, tag = "11")]
    pub encrypted_data2: Option<Vec<u8>>,
}

/// `rpc.OutgoingRPCData`: the body of an outgoing request, which carries the encrypted params.
#[derive(Clone, PartialEq, prost::Message)]
pub struct OutgoingRpcData {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(int32, tag = "2")]
    pub action: i32,
    #[prost(bytes = "vec", tag = "3")]
    pub unencrypted_proto_data: Vec<u8>,
    #[prost(bytes = "vec", tag = "5")]
    pub encrypted_proto_data: Vec<u8>,
    #[prost(string, tag = "6")]
    pub session_id: String,
}

/// `rpc.RPCPairData`: pairing events on the pair route.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RpcPairData {
    #[prost(oneof = "rpc_pair_data::Event", tags = "4, 5")]
    pub event: Option<rpc_pair_data::Event>,
}

/// Oneof of [`RpcPairData`].
pub mod rpc_pair_data {
    /// The `event` oneof.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "4")]
        Paired(super::PairedData),
        #[prost(message, tag = "5")]
        Revoked(super::RevokePairData),
    }
}

/// `rpc.RPCGaiaData`: Google account pairing events and the device list of `SignInGaia`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RpcGaiaData {
    #[prost(int32, tag = "1")]
    pub command: i32,
    #[prost(message, optional, tag = "108")]
    pub maybe_server_data: Option<rpc_gaia_data::UnknownContainer>,
}

/// Nested messages of [`RpcGaiaData`].
pub mod rpc_gaia_data {
    /// `RPCGaiaData.UnknownContainer`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct UnknownContainer {
        #[prost(message, optional, tag = "2")]
        pub item2: Option<Item2>,
        #[prost(int64, tag = "3")]
        pub unknown_timestamp_microseconds: i64,
        #[prost(message, repeated, tag = "4")]
        pub item4: Vec<Item4>,
    }

    /// `RPCGaiaData.UnknownContainer.Item2`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Item2 {
        #[prost(message, repeated, tag = "1")]
        pub item1: Vec<Item2Item1>,
    }

    /// `RPCGaiaData.UnknownContainer.Item2.Item1`: one device the account can pair with.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Item2Item1 {
        #[prost(string, tag = "1")]
        pub dest_or_source_uuid: String,
        #[prost(int32, tag = "4")]
        pub unknown_int4: i32,
        #[prost(string, tag = "5")]
        pub language_code: String,
        #[prost(uint64, tag = "7")]
        pub unknown_big_int7: u64,
    }

    /// `RPCGaiaData.UnknownContainer.Item4`: presence and timing of a device.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Item4 {
        #[prost(string, tag = "1")]
        pub dest_or_source_uuid: String,
        #[prost(int32, tag = "3")]
        pub unknown_int3: i32,
        #[prost(int32, tag = "4")]
        pub unknown_int4: i32,
        #[prost(int64, tag = "7")]
        pub unknown_timestamp_microseconds: i64,
        #[prost(message, optional, tag = "8")]
        pub item8: Option<Item8>,
    }

    /// `RPCGaiaData.UnknownContainer.Item4.Item8`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Item8 {
        #[prost(int32, tag = "1")]
        pub unknown_int1: i32,
        #[prost(uint64, tag = "2")]
        pub unknown_timestamp: u64,
        #[prost(bytes = "vec", tag = "3")]
        pub unknown_bytes: Vec<u8>,
        #[prost(int32, tag = "4")]
        pub unknown_int2: i32,
    }
}
