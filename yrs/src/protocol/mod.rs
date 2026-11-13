//! The y-sync protocol and awareness: the messages peers exchange to sync documents and
//! presence. Behind the `protocol` feature (on by default).

/// Awareness: a record of the state of each client, such as the user's presence, that's synced between peers.
pub mod awareness;
/// The messages of the y-sync protocol and their handling.
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
