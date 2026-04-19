//! Stable object identifiers.
//!
//! `ObjectId` is the scene graph's primary key. Edits, animations,
//! and external operations reference objects by ID so their identity
//! survives reordering, cloning, or persistence.

use std::fmt;

/// Opaque scene-object identifier.
///
/// Backed by a `u64` for cheap `Copy` and deterministic ordering, but
/// the value carries no semantic meaning — don't derive indices from
/// it. Construct via [`ObjectId::new`] or the `Default` ("none")
/// sentinel. Scene builders typically allocate IDs via a
/// monotonically-incrementing counter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(u64);

impl ObjectId {
    /// Build an ID from a raw value. Two IDs with the same raw value
    /// compare equal. `0` is reserved for the `Default` sentinel.
    pub const fn new(raw: u64) -> Self {
        ObjectId(raw)
    }

    /// Return the raw backing value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Whether this is the `Default` sentinel (raw `0`, logically
    /// "no object").
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "obj#{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_none_sentinel() {
        assert!(ObjectId::default().is_none());
        assert_eq!(ObjectId::default().raw(), 0);
    }

    #[test]
    fn non_zero_is_not_none() {
        let id = ObjectId::new(42);
        assert!(!id.is_none());
        assert_eq!(id.raw(), 42);
    }

    #[test]
    fn display_format() {
        assert_eq!(ObjectId::new(7).to_string(), "obj#7");
    }
}
