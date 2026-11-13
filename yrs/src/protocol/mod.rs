//! The y-sync protocol and awareness: the messages peers exchange to sync documents and
//! presence. Behind the `protocol` feature (on by default).

pub mod awareness;
#[allow(clippy::module_inception)]
pub mod protocol;
pub use crate::time;

pub use crate::protocol::awareness::Awareness;
pub use crate::protocol::awareness::AwarenessUpdate;
pub use crate::protocol::protocol::DefaultProtocol;
pub use crate::protocol::protocol::Error;
pub use crate::protocol::protocol::Message;
pub use crate::protocol::protocol::MessageReader;
pub use crate::protocol::protocol::Protocol;
pub use crate::protocol::protocol::SyncMessage;
pub use crate::time::Clock;
pub use crate::time::Timestamp;
