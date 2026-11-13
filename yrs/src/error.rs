use crate::ID;
use thiserror::Error;

/// An error from decoding or applying an update.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The update couldn't be decoded.
    #[error("{0}")]
    ReadError(#[from] crate::encoding::read::Error),
    /// The update couldn't be applied.
    #[error("failed to apply update: {0}")]
    UpdateError(#[from] UpdateError),
    /// The operation can't be executed on a document that has garbage collection enabled.
    #[error("Cannot execute this operation when document garbage collection is set")]
    Gc,
}

/// An update that decoded but can't be applied.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UpdateError {
    /// A block names the item with this ID as its parent, but that item holds neither a shared
    /// type nor deleted content. The second value is the content tag it holds instead.
    #[error("block parent {0} must be deleted or shared ref type. Type: {1}")]
    InvalidParent(ID, u8),
}

/// A position or range that doesn't fit in a sequence: an insert past its end, or a removal or
/// formatting range that runs past it. Returned by the `try_*` methods of
/// [crate::Text], [crate::Array] and [crate::XmlFragment], before anything is changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
#[error("range {index}..{end} is out of bounds for length {available}", end = *.index as u64 + *.len as u64)]
pub struct OutOfRange {
    /// Where the operation starts.
    pub index: u32,
    /// How many elements it covers (0 for an insert).
    pub len: u32,
    /// The length of the sequence, in the units `index` and `len` count.
    pub available: u32,
}

impl OutOfRange {
    /// Checks that `index..index + len` fits in a sequence of `available` elements.
    ///
    /// # Errors
    ///
    /// [OutOfRange] describing the range, if it doesn't fit.
    pub fn check(available: u32, index: u32, len: u32) -> Result<(), OutOfRange> {
        match index.checked_add(len) {
            Some(end) if end <= available => Ok(()),
            _ => Err(OutOfRange {
                index,
                len,
                available,
            }),
        }
    }
}
