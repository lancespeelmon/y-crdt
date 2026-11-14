//! A string that keeps up to 8 bytes inline, for the short strings most text items hold.

use smallvec::SmallVec;
use std::fmt;

/// UTF-8 text in a [SmallVec], inline up to 8 bytes. Every byte sequence it holds came from a
/// `&str`, so it is always valid UTF-8.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SmallStr(SmallVec<[u8; 8]>);

impl SmallStr {
    #[inline]
    pub fn from_str(s: &str) -> Self {
        SmallStr(SmallVec::from_slice(s.as_bytes()))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: the bytes only ever come from `&str` values (`from_str`, `push_str`), which
        // are valid UTF-8, and nothing else mutates them.
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    #[inline]
    pub fn push_str(&mut self, s: &str) {
        self.0.extend_from_slice(s.as_bytes());
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the string outgrew the inline buffer.
    #[inline]
    pub fn spilled(&self) -> bool {
        self.0.spilled()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub fn into_string(self) -> String {
        // SAFETY: see `as_str`.
        unsafe { String::from_utf8_unchecked(self.0.into_vec()) }
    }
}

impl std::ops::Deref for SmallStr {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for SmallStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for SmallStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl From<&str> for SmallStr {
    #[inline]
    fn from(s: &str) -> Self {
        SmallStr::from_str(s)
    }
}

impl From<String> for SmallStr {
    #[inline]
    fn from(s: String) -> Self {
        // A short string goes inline; a long one keeps its allocation.
        if s.len() <= 8 {
            SmallStr::from_str(&s)
        } else {
            SmallStr(SmallVec::from_vec(s.into_bytes()))
        }
    }
}

#[cfg(test)]
mod test {
    use super::SmallStr;

    #[test]
    fn short_strings_stay_inline_and_long_ones_spill() {
        let mut s = SmallStr::from_str("abc");
        assert!(!s.spilled());
        s.push_str("défghij");
        assert!(s.spilled());
        assert_eq!(s.as_str(), "abcdéfghij");
        assert_eq!(s.len(), 11);
        assert_eq!(SmallStr::from("x".repeat(20)).into_string(), "x".repeat(20));
        assert!(SmallStr::from_str("a") < SmallStr::from_str("b"));
        // The same size as smallstr's `SmallString<[u8; 8]>`, which wrapped the same `SmallVec`.
        #[cfg(target_pointer_width = "64")]
        assert_eq!(std::mem::size_of::<SmallStr>(), 24);
    }
}
