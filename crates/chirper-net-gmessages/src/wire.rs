//! The `pblite` envelopes of the Messages web endpoints: the requests the driver sends (`SendMessage`,
//! `AckMessages`, `ReceiveMessages`, `SignInGaia`, `RegisterRefresh`) and the long-poll payloads and
//! responses it reads back. Field numbers follow `libgm/gmproto`; the layout of each message is
//! written out next to its encoder or decoder.
//!
//! Inner payloads (`messageData`, the encrypted action bodies) are binary protobuf inside a pblite
//! bytes field, and are decoded by the callers with `prost`.

use serde_json::Value;

use chirper_bridge::{Error, Result};

use crate::pblite::{Array, Fields};
use crate::proto::pairing::{
    AuthMessage, ConfigVersion, Device, IncomingRpcMessage, SignInGaiaResponse, TokenData,
    incoming_rpc_message::GdittoSource, rpc_gaia_data::Item2Item1, rpc_gaia_data::Item4,
    rpc_gaia_data::Item8, sign_in_gaia_request, sign_in_gaia_response,
};

/// One item of a long-poll stream.
#[derive(Debug)]
pub enum LongPollItem {
    /// An event or a response from the phone.
    Data(Box<IncomingRpcMessage>),
    /// The server's ack count for the items it replays after a reconnect.
    Ack(i32),
    /// A keep-alive.
    Heartbeat,
    /// The server is ready to send.
    StartRead,
    /// Something this driver does not read.
    Unknown,
}

/// `Device`: `userID` (1), `sourceID` (2), `network` (3).
pub fn device_to_pblite(device: &Device) -> Array {
    let mut arr = Array::new();
    arr.int(1, device.user_id)
        .string(2, &device.source_id)
        .string(3, &device.network);
    arr
}

/// Read a `Device`.
pub fn device_from_pblite(fields: &Fields<'_>) -> Result<Device> {
    Ok(Device {
        user_id: fields.int(1)?,
        source_id: fields.string(2)?,
        network: fields.string(3)?,
    })
}

fn config_version_to_pblite(version: &ConfigVersion) -> Array {
    let mut arr = Array::new();
    arr.int(3, i64::from(version.year))
        .int(4, i64::from(version.month))
        .int(5, i64::from(version.day))
        .int(7, i64::from(version.v1))
        .int(9, i64::from(version.v2));
    arr
}

/// `AuthMessage`: `requestID` (1), `network` (3), `tachyonAuthToken` (6), `configVersion` (7).
pub fn auth_to_pblite(auth: &AuthMessage) -> Array {
    let mut arr = Array::new();
    arr.string(1, &auth.request_id);
    if !auth.network.is_empty() {
        arr.string(3, &auth.network);
    }
    if !auth.tachyon_auth_token.is_empty() {
        arr.bytes(6, &auth.tachyon_auth_token);
    }
    if let Some(version) = &auth.config_version {
        arr.message(7, config_version_to_pblite(version));
    }
    arr
}

/// `TokenData`: `tachyonAuthToken` (1), `TTL` (2).
pub fn token_data_from_pblite(fields: &Fields<'_>) -> Result<TokenData> {
    Ok(TokenData {
        tachyon_auth_token: fields.bytes(1)?,
        ttl: fields.int(2)?,
    })
}

/// The `ReceiveMessages` request: the auth header plus the empty `unknown` object the server asks for.
pub fn receive_messages_request(auth: &AuthMessage) -> Value {
    let mut empty_inner = Array::new();
    empty_inner.set(2, Value::Array(Vec::new()));
    let mut arr = Array::new();
    arr.message(1, auth_to_pblite(auth));
    arr.message(4, empty_inner);
    arr.into_value()
}

/// The `AckMessages` request: the request ids to acknowledge, each with the device it came from.
pub fn ack_request(auth: &AuthMessage, acks: &[(String, Option<Device>)]) -> Value {
    let entries = acks
        .iter()
        .map(|(request_id, device)| {
            let mut entry = Array::new();
            entry.string(1, request_id);
            if let Some(device) = device {
                entry.message(2, device_to_pblite(device));
            }
            entry
        })
        .collect();
    let mut arr = Array::new();
    arr.message(1, auth_to_pblite(auth));
    arr.set(2, Value::Array(Vec::new()));
    arr.list(4, entries);
    arr.into_value()
}

/// The fields of an outgoing RPC (`OutgoingRPCMessage` in `rpc.proto`).
#[derive(Debug)]
pub struct OutgoingRpc<'a> {
    pub mobile: Option<&'a Device>,
    pub request_id: &'a str,
    pub bugle_route: i32,
    pub message_type: i32,
    /// The `OutgoingRPCData` bytes.
    pub message_data: &'a [u8],
    pub tachyon_auth_token: &'a [u8],
    pub config_version: &'a ConfigVersion,
    /// Time to live in microseconds; 0 leaves it out.
    pub ttl: i64,
    /// Destination registration ids; each is sent base64 encoded (`pblite_binary`).
    pub dest_registration_ids: &'a [String],
}

/// Encode an [`OutgoingRpc`] for `SendMessage`.
///
/// `OutgoingRPCMessage`: `mobile` (1), `data` (2) `{requestID 1, bugleRoute 2, messageData 12,
/// messageTypeData 23 {emptyArr 1, messageType 2}}`, `auth` (3) `{requestID 1,
/// tachyonAuthToken 6, configVersion 7}`, `TTL` (5), `destRegistrationIDs` (9).
pub fn outgoing_rpc_to_pblite(rpc: &OutgoingRpc<'_>) -> Value {
    let mut data = Array::new();
    data.string(1, rpc.request_id)
        .int(2, i64::from(rpc.bugle_route))
        .bytes(12, rpc.message_data);
    let mut type_data = Array::new();
    type_data.set(1, Value::Array(Vec::new()));
    type_data.int(2, i64::from(rpc.message_type));
    data.message(23, type_data);

    let mut auth = Array::new();
    auth.string(1, rpc.request_id)
        .bytes(6, rpc.tachyon_auth_token)
        .message(7, config_version_to_pblite(rpc.config_version));

    let mut arr = Array::new();
    if let Some(mobile) = rpc.mobile {
        arr.message(1, device_to_pblite(mobile));
    }
    arr.message(2, data).message(3, auth);
    if rpc.ttl != 0 {
        arr.int(5, rpc.ttl);
    }
    if !rpc.dest_registration_ids.is_empty() {
        let ids = rpc
            .dest_registration_ids
            .iter()
            .map(|id| Value::String(crate::pblite::base64_encode(id.as_bytes())))
            .collect();
        arr.set(9, Value::Array(ids));
    }
    arr.into_value()
}

/// Read one block of a long-poll stream: `LongPollingPayload` with `data` (2), `heartbeat` (3),
/// `ack` (4, `StartAckMessage {count 1}`) and `startRead` (5).
pub fn long_poll_item(value: &Value) -> Result<LongPollItem> {
    let fields = Fields::of(value)?;
    if let Some(data) = fields.message(2)? {
        return Ok(LongPollItem::Data(Box::new(incoming_from_pblite(&data)?)));
    }
    if let Some(ack) = fields.message(4)? {
        let count = ack.int(1)?;
        return Ok(LongPollItem::Ack(i32::try_from(count).unwrap_or(0)));
    }
    if fields.get(3).is_some() {
        return Ok(LongPollItem::Heartbeat);
    }
    if fields.get(5).is_some() {
        return Ok(LongPollItem::StartRead);
    }
    Ok(LongPollItem::Unknown)
}

/// Read an `IncomingRPCMessage`.
pub fn incoming_from_pblite(fields: &Fields<'_>) -> Result<IncomingRpcMessage> {
    let gditto_source = match fields.message(23)? {
        Some(inner) => Some(GdittoSource {
            device_id: i32::try_from(inner.int(2)?)
                .map_err(|_| Error::Protocol("a device id is out of range".to_owned()))?,
        }),
        None => None,
    };
    Ok(IncomingRpcMessage {
        response_id: fields.string(1)?,
        bugle_route: to_i32(fields.int(2)?)?,
        start_execute: fields.int(3)?.try_into().unwrap_or(0),
        message_type: to_i32(fields.int(5)?)?,
        finish_execute: fields.int(6)?.try_into().unwrap_or(0),
        microseconds_taken: fields.int(7)?.try_into().unwrap_or(0),
        mobile: match fields.message(8)? {
            Some(device) => Some(device_from_pblite(&device)?),
            None => None,
        },
        browser: match fields.message(9)? {
            Some(device) => Some(device_from_pblite(&device)?),
            None => None,
        },
        message_data: fields.bytes(12)?,
        signature_id: fields.string(17)?,
        timestamp: fields.string(21)?,
        gditto_source,
    })
}

fn to_i32(value: i64) -> Result<i32> {
    i32::try_from(value).map_err(|_| Error::Protocol("an enum value is out of range".to_owned()))
}

/// `SignInGaiaRequest`: `authMessage` (1), `inner` (2) `{deviceID 1 {unknownInt1 1, deviceID 2},
/// someData 36 (binary, `Data {someData 3}`)}`, `unknownInt3` (3), `network` (4).
pub fn sign_in_gaia_request_to_pblite(
    auth: &AuthMessage,
    device_id: &str,
    public_key: Option<&[u8]>,
    unknown_int3: i64,
    network: &str,
) -> Value {
    let mut device = Array::new();
    device.int(1, 3).string(2, device_id);
    let mut inner = Array::new();
    inner.message(1, device);
    if let Some(key) = public_key {
        let data = sign_in_gaia_request::Data {
            some_data: key.to_vec(),
        };
        let binary = prost::Message::encode_to_vec(&data);
        inner.set(36, Value::String(crate::pblite::base64_encode(&binary)));
    }
    let mut arr = Array::new();
    arr.message(1, auth_to_pblite(auth))
        .message(2, inner)
        .int(3, unknown_int3)
        .string(4, network);
    arr.into_value()
}

/// Read a `SignInGaiaResponse`: `header` (1), `maybeBrowserUUID` (2, base64), `deviceData` (3),
/// `tokenData` (4).
pub fn sign_in_gaia_response_from_pblite(value: &Value) -> Result<SignInGaiaResponse> {
    let fields = Fields::of(value)?;
    let header = match fields.message(1)? {
        Some(header) => Some(sign_in_gaia_response::Header {
            unknown_int2: u64::try_from(header.int(2)?).unwrap_or(0),
            unknown_timestamp: header.int(4)?,
        }),
        None => None,
    };
    let device_data = match fields.message(3)? {
        Some(data) => Some(read_device_data(&data)?),
        None => None,
    };
    Ok(SignInGaiaResponse {
        header,
        maybe_browser_uuid: decode_b64_string(&fields, 2)?,
        device_data,
        token_data: match fields.message(4)? {
            Some(token) => Some(token_data_from_pblite(&token)?),
            None => None,
        },
    })
}

/// `SignInGaiaResponse.DeviceData`: `deviceWrapper` (1), `unknownItems2` (2, `Item2.Item1` list),
/// `unknownItems3` (3, `Item4` list).
fn read_device_data(fields: &Fields<'_>) -> Result<sign_in_gaia_response::DeviceData> {
    let device_wrapper = match fields.message(1)? {
        Some(wrapper) => Some(sign_in_gaia_response::DeviceWrapper {
            device: match wrapper.message(1)? {
                Some(device) => Some(device_from_pblite(&device)?),
                None => None,
            },
        }),
        None => None,
    };
    let mut unknown_items2 = Vec::new();
    for entry in fields.list(2)? {
        unknown_items2.push(Item2Item1 {
            dest_or_source_uuid: decode_b64_string(&entry, 1)?,
            unknown_int4: to_i32(entry.int(4)?)?,
            language_code: entry.string(5)?,
            unknown_big_int7: u64::try_from(entry.int(7)?).unwrap_or(0),
        });
    }
    let mut unknown_items3 = Vec::new();
    for entry in fields.list(3)? {
        unknown_items3.push(read_item4(&entry)?);
    }
    Ok(sign_in_gaia_response::DeviceData {
        device_wrapper,
        unknown_items2,
        unknown_items3,
    })
}

/// `RPCGaiaData.UnknownContainer.Item4`, whose `item8` is base64 of its binary protobuf.
fn read_item4(entry: &Fields<'_>) -> Result<Item4> {
    let item8 = match entry.get(8) {
        Some(Value::String(encoded)) => {
            let bytes = crate::pblite::base64_decode(encoded)?;
            Some(
                <Item8 as prost::Message>::decode(bytes.as_slice())
                    .map_err(|_| Error::Protocol("a device status is not valid protobuf".to_owned()))?,
            )
        }
        _ => None,
    };
    Ok(Item4 {
        dest_or_source_uuid: decode_b64_string(entry, 1)?,
        unknown_int3: to_i32(entry.int(3)?)?,
        unknown_int4: to_i32(entry.int(4)?)?,
        unknown_timestamp_microseconds: entry.int(7)?,
        item8,
    })
}

/// A string field the schema marks `pblite_binary`: base64 of its bytes. Absent reads as empty.
fn decode_b64_string(fields: &Fields<'_>, field: usize) -> Result<String> {
    match fields.get(field) {
        None => Ok(String::new()),
        Some(Value::String(encoded)) => {
            let bytes = crate::pblite::base64_decode(encoded)?;
            String::from_utf8(bytes)
                .map_err(|_| Error::Protocol("a pblite string is not UTF-8".to_owned()))
        }
        Some(_) => Err(Error::Protocol(format!("pblite field {field} is not a string"))),
    }
}

/// The push endpoint a token refresh may register (`RegisterRefreshRequest.PushRegistration`).
#[derive(Clone, Debug, Default)]
pub struct PushRegistration {
    pub url: String,
    pub p256dh: String,
    pub auth: String,
}

/// `RegisterRefreshRequest`: `messageAuth` (1), `currBrowserDevice` (2), `unixTimestamp` (3),
/// `signature` (4), `parameters` (13) `{emptyArr 9, moreParameters 23 {three 1, pushReg 102}}`,
/// `messageType` (16).
pub fn register_refresh_request_to_pblite(
    auth: &AuthMessage,
    browser: &Device,
    timestamp_us: i64,
    signature: &[u8],
    push: Option<&PushRegistration>,
) -> Value {
    let mut parameters = Array::new();
    parameters.set(9, Value::Array(Vec::new()));
    if let Some(push) = push {
        let mut reg = Array::new();
        reg.string(1, "messages_web")
            .string(2, &push.url)
            .string(3, &push.p256dh)
            .string(4, &push.auth);
        let mut more = Array::new();
        more.int(1, 3).message(102, reg);
        parameters.message(23, more);
    }
    let mut arr = Array::new();
    arr.message(1, auth_to_pblite(auth))
        .message(2, device_to_pblite(browser))
        .int(3, timestamp_us)
        .bytes(4, signature)
        .message(13, parameters)
        .int(16, 2);
    arr.into_value()
}

/// `RegisterRefreshResponse.tokenData` (field 2).
pub fn register_refresh_response_from_pblite(value: &Value) -> Result<Option<TokenData>> {
    let fields = Fields::of(value)?;
    match fields.message(2)? {
        Some(token) => Ok(Some(token_data_from_pblite(&token)?)),
        None => Ok(None),
    }
}
