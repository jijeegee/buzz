#![deny(unsafe_code)]

pub mod connection;
pub mod error;
pub mod message;

pub use connection::{
    publish_event, publish_event_with_device, publish_event_with_token, NostrWsConnection,
};
pub use error::WsClientError;
pub use message::{
    apply_device_tag, build_auth_event, parse_relay_message, unsigned_auth_event, DeviceTagger,
    OkResponse, RelayMessage,
};
