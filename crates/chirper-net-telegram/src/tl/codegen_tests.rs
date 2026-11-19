//! Tests of the generated TL code against byte layouts taken from the schema.

use super::functions::{self, InitConnection, InvokeWithLayer};
use super::types;
use super::{Cursor, Deserializable, Serializable};

fn le(id: u32) -> [u8; 4] {
    id.to_le_bytes()
}

#[test]
fn invoke_with_layer_of_init_connection_of_get_config_starts_with_the_right_ids() {
    let request = InvokeWithLayer {
        layer: super::LAYER,
        query: InitConnection {
            api_id: 1,
            device_model: "model".to_owned(),
            system_version: "system".to_owned(),
            app_version: "app".to_owned(),
            system_lang_code: "en".to_owned(),
            lang_pack: String::new(),
            lang_code: "en".to_owned(),
            proxy: None,
            params: None,
            query: functions::help::GetConfig {},
        },
    };
    let mut buf = Vec::new();
    request.serialize(&mut buf);
    assert_eq!(&buf[..4], &le(0xda9b_0d0d), "invokeWithLayer id");
    assert_eq!(&buf[4..8], &le(233), "layer");
    assert_eq!(&buf[8..12], &le(0xc1cd_5ea9), "initConnection id");
    // The flags word of initConnection has no bits set (no proxy, no params).
    assert_eq!(&buf[12..16], &le(0), "flags");
    // help.getConfig is the last thing written: its bare id.
    assert_eq!(&buf[buf.len() - 4..], &le(0xc4f9_186b), "help.getConfig id");
}

#[test]
fn a_function_without_fields_serialises_to_its_id() {
    let mut buf = Vec::new();
    functions::help::GetNearestDc {}.serialize(&mut buf);
    assert_eq!(buf, le(0x1fb3_3026));
}

#[test]
fn flags_bits_follow_the_presence_of_optional_fields() {
    // inputMediaPhotoExternal: spoiler is flags.1?true, ttl_seconds is flags.0?int.
    let absent = types::InputMediaPhotoExternal {
        spoiler: false,
        url: "u".to_owned(),
        ttl_seconds: None,
    };
    let mut buf = Vec::new();
    absent.serialize(&mut buf);
    assert_eq!(&buf[..4], &le(0xe5bb_fe1a));
    assert_eq!(&buf[4..8], &le(0), "no flag bits set");

    let present = types::InputMediaPhotoExternal {
        spoiler: true,
        url: "u".to_owned(),
        ttl_seconds: Some(5),
    };
    let mut buf = Vec::new();
    present.serialize(&mut buf);
    assert_eq!(&buf[4..8], &le(0b11), "bit 0 for ttl, bit 1 for spoiler");
    // flags, then url (length 1, 'u', two pad bytes), then ttl.
    assert_eq!(&buf[8..], &[1, b'u', 0, 0, 5, 0, 0, 0][..]);
}

#[test]
fn flagged_struct_round_trips_through_its_boxed_enum() {
    let original = types::InputMediaPhotoExternal {
        spoiler: true,
        url: "https://example.org/a.jpg".to_owned(),
        ttl_seconds: Some(60),
    };
    let mut buf = Vec::new();
    original.serialize(&mut buf);
    let mut cursor = Cursor::new(&buf);
    let decoded = super::enums::InputMedia::deserialize(&mut cursor).unwrap();
    assert_eq!(cursor.remaining(), 0);
    assert_eq!(
        decoded,
        super::enums::InputMedia::InputMediaPhotoExternal(original)
    );
}

#[test]
fn a_boxed_result_round_trips() {
    let value = types::NearestDc {
        country: "NL".to_owned(),
        this_dc: 2,
        nearest_dc: 4,
    };
    let mut buf = Vec::new();
    value.serialize(&mut buf);
    let mut cursor = Cursor::new(&buf);
    let decoded = super::enums::NearestDc::deserialize(&mut cursor).unwrap();
    assert_eq!(decoded, super::enums::NearestDc::NearestDc(value));
}

#[test]
fn unknown_constructor_in_a_boxed_type_is_an_error() {
    let buf = le(0x1234_5678);
    let mut cursor = Cursor::new(&buf);
    let result = super::enums::NearestDc::deserialize(&mut cursor);
    assert_eq!(
        result,
        Err(super::DeserializeError::UnknownConstructor(0x1234_5678))
    );
}

#[test]
fn truncated_struct_is_an_error_not_a_panic() {
    let value = types::NearestDc {
        country: "NL".to_owned(),
        this_dc: 2,
        nearest_dc: 4,
    };
    let mut buf = Vec::new();
    value.serialize(&mut buf);
    for cut in 0..buf.len() {
        let mut cursor = Cursor::new(&buf[..cut]);
        assert!(
            super::enums::NearestDc::deserialize(&mut cursor).is_err(),
            "cut {cut}"
        );
    }
}
