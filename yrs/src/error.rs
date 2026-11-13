use crate::ID;
use thiserror::Error;

/// An error from decoding or applying an update.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("{0}")]
    ReadError(#[from] crate::encoding::read::Error),
    #[error("failed to apply update: {0}")]
    UpdateError(#[from] UpdateError),
    #[error("Cannot execute this operation when document garbage collection is set")]
    Gc,
}

/// An update that decoded but can't be applied.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UpdateError {
    #[error("block parent {0} must be deleted or shared ref type. Type: {1}")]
    InvalidParent(ID, u8),
}
