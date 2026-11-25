//! The `invokeWithLayer(initConnection(...))` wrapper and the device fields it carries.
//!
//! The wrapper is written by hand (rather than through the generated `InitConnection<X>`)
//! because the generated type owns its query and a call only has a borrow of it.

use crate::tl::{RemoteCall, Serializable, write_bytes};

/// Constructor id of `invokeWithLayer` (`#da9b0d0d` in the schema).
pub const INVOKE_WITH_LAYER_ID: u32 = 0xda9b_0d0d;
/// Constructor id of `initConnection` (`#c1cd5ea9` in the schema).
pub const INIT_CONNECTION_ID: u32 = 0xc1cd_5ea9;

/// The device the session reports to Telegram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub model: String,
    pub system: String,
    pub app: String,
    pub lang: String,
}

impl Device {
    /// The device this build reports: model "Chirper", the operating system, the crate version.
    pub fn current() -> Self {
        Self {
            model: "Chirper".to_owned(),
            system: std::env::consts::OS.to_owned(),
            app: env!("CARGO_PKG_VERSION").to_owned(),
            lang: "en".to_owned(),
        }
    }
}

/// The fields of `initConnection` other than the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitFields {
    pub api_id: i32,
    pub device: Device,
}

impl InitFields {
    pub fn new(api_id: i32, device: Device) -> Self {
        Self { api_id, device }
    }
}

/// A request sent as `invokeWithLayer(initConnection(request))`.
pub struct Wrapped<'a, R> {
    init: &'a InitFields,
    request: &'a R,
}

impl<'a, R> Wrapped<'a, R> {
    pub fn new(init: &'a InitFields, request: &'a R) -> Self {
        Self { init, request }
    }
}

impl<R: RemoteCall> Serializable for Wrapped<'_, R> {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&INVOKE_WITH_LAYER_ID.to_le_bytes());
        buf.extend_from_slice(&crate::tl::LAYER.to_le_bytes());
        buf.extend_from_slice(&INIT_CONNECTION_ID.to_le_bytes());
        // No proxy and no params: an empty flags word.
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&self.init.api_id.to_le_bytes());
        write_bytes(buf, self.init.device.model.as_bytes());
        write_bytes(buf, self.init.device.system.as_bytes());
        write_bytes(buf, self.init.device.app.as_bytes());
        write_bytes(buf, self.init.device.lang.as_bytes());
        write_bytes(buf, b"");
        write_bytes(buf, self.init.device.lang.as_bytes());
        self.request.serialize(buf);
    }
}

impl<R: RemoteCall> RemoteCall for Wrapped<'_, R> {
    type Return = R::Return;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tl::Identifiable;
    use crate::tl::functions::help::GetConfig;

    #[test]
    fn wrapper_ids_match_the_schema() {
        assert_eq!(
            <crate::tl::functions::InvokeWithLayer<GetConfig> as Identifiable>::CONSTRUCTOR_ID,
            INVOKE_WITH_LAYER_ID
        );
        assert_eq!(
            <crate::tl::functions::InitConnection<GetConfig> as Identifiable>::CONSTRUCTOR_ID,
            INIT_CONNECTION_ID
        );
    }

    #[test]
    fn wrapper_carries_the_layer_and_the_request() {
        let init = InitFields::new(1, Device::current());
        let request = GetConfig {};
        let mut buf = Vec::new();
        Wrapped::new(&init, &request).serialize(&mut buf);
        assert_eq!(buf.get(..4), Some(&INVOKE_WITH_LAYER_ID.to_le_bytes()[..]));
        assert_eq!(buf.get(4..8), Some(&crate::tl::LAYER.to_le_bytes()[..]));
        assert_eq!(buf.get(8..12), Some(&INIT_CONNECTION_ID.to_le_bytes()[..]));
        let mut request_bytes = Vec::new();
        request.serialize(&mut request_bytes);
        assert!(buf.ends_with(&request_bytes));
    }
}
