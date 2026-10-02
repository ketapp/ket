//! Stable identifiers for the entities ket tracks.
//!
//! These are newtypes rather than bare `String`s so a project id cannot be passed
//! where a worktree id is expected. With many projects in flight simultaneously,
//! that mistake is easy to make and completely silent when everything is a string.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Identifies a project — a git repository ket knows about.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

/// Identifies a worktree belonging to a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorktreeId(String);

/// Identifies a single run of an agent against a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

macro_rules! impl_id {
    ($ty:ident) => {
        impl $ty {
            /// Wraps an already-formed identifier.
            pub fn new(raw: impl Into<String>) -> Self {
                Self(raw.into())
            }

            /// The identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // `pad`, not `write_str`: the latter silently ignores width and
                // alignment, which turns every `{:<28}` in a listing into no
                // padding at all.
                f.pad(&self.0)
            }
        }

        impl From<&str> for $ty {
            fn from(raw: &str) -> Self {
                Self(raw.to_owned())
            }
        }
    };
}

impl_id!(ProjectId);
impl_id!(WorktreeId);
impl_id!(SessionId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialize_as_bare_strings() {
        // The wire format matters: TypeScript should see `"proj-a"`, not
        // `{"0":"proj-a"}`. `#[serde(transparent)]` is what guarantees that.
        let json = serde_json::to_string(&ProjectId::new("proj-a")).unwrap();
        assert_eq!(json, r#""proj-a""#);
    }

    #[test]
    fn ids_honour_formatting_width() {
        // The CLI lays out columns with `{:<28}`. A `Display` that writes the
        // string directly ignores that, and every listing comes out ragged.
        assert_eq!(format!("[{:<10}]", ProjectId::new("api")), "[api       ]");
    }

    #[test]
    fn ids_round_trip() {
        let id = WorktreeId::new("wt-1");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<WorktreeId>(&json).unwrap(), id);
    }
}
