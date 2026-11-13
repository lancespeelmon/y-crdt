use std::fmt::{Display, Formatter};

mod iter_any;
mod iter_txn;
mod parse;

pub use iter_txn::JsonPathIter;

/// Trait implemented by types capable of evaluating JSON Paths.
pub trait JsonPathEval {
    /// The type of the iterator over the values that match a path.
    type Iter<'a>
    where
        Self: 'a;
    /// Returns an iterator over the values that match the given `path`.
    fn json_path<'a>(&'a self, path: &'a JsonPath<'a>) -> Self::Iter<'a>;
}

/// A JSON path query. Use [JsonPath::parse] to create one.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPath<'a> {
    tokens: Vec<JsonPathToken<'a>>,
}

impl<'a> AsRef<[JsonPathToken<'a>]> for JsonPath<'a> {
    fn as_ref(&self) -> &[JsonPathToken<'a>] {
        &self.tokens
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum JsonPathToken<'a> {
    Root,
    Current,
    Member(&'a str),
    Index(i32),
    Wildcard,
    RecursiveDescend,
    Slice(u32, u32, u32),
    MemberUnion(Vec<&'a str>),
    IndexUnion(Vec<i32>),
}

impl<'a> Display for JsonPathToken<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonPathToken::Root => write!(f, r#"$"#),
            JsonPathToken::Current => write!(f, "@"),
            JsonPathToken::Member(key) => {
                if key.chars().any(char::is_whitespace) {
                    write!(f, "['{}']", key)
                } else {
                    write!(f, ".{}", key)
                }
            }
            JsonPathToken::Index(index) => write!(f, "[{}]", index),
            JsonPathToken::Wildcard => write!(f, ".*"),
            JsonPathToken::RecursiveDescend => write!(f, ".."),
            JsonPathToken::Slice(from, to, by) => write!(f, "[{}:{}:{}]", from, to, by),
            JsonPathToken::MemberUnion(members) => {
                let mut i = members.iter();
                write!(f, "[")?;
                if let Some(m) = i.next() {
                    write!(f, "{}", m)?;
                }
                while let Some(m) = i.next() {
                    write!(f, ", {}", m)?;
                }
                write!(f, "]")
            }
            JsonPathToken::IndexUnion(indices) => {
                let mut i = indices.iter();
                write!(f, "[")?;
                if let Some(m) = i.next() {
                    write!(f, "{}", m)?;
                }
                while let Some(m) = i.next() {
                    write!(f, ", {}", m)?;
                }
                write!(f, "]")
            }
        }
    }
}

/// An error from parsing a JSON path.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The string isn't a valid JSON path. It describes why.
    #[error("{0}")]
    InvalidJsonPath(String),
}
