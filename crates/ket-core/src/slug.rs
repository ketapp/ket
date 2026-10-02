//! Turning arbitrary names into safe, non-colliding path components.
//!
//! Git branch names are close to unrestricted: they contain `/`, dots, unicode,
//! and can differ only by case. Filesystem path components are not. Using a
//! branch name directly as a directory name is the bug this module exists to
//! prevent — `feature/login` would silently nest a directory *and* collide with
//! a branch literally named `feature`.
//!
//! A slug is therefore two parts: a readable prefix for humans scanning a
//! directory listing, and a hash suffix that does the actual disambiguating.
//! The readable part may collide freely; the hash is what guarantees it does not
//! matter.

use std::fmt::Write as _;

/// Longest readable prefix kept before the hash.
///
/// APFS allows 255 bytes per component. This is far below that, because the
/// prefix is a convenience for humans, not an identifier.
const MAX_READABLE: usize = 40;

/// Hex characters of hash appended to every slug.
///
/// 48 bits. For the hundreds of branches a person might accumulate, collision
/// probability is negligible, and a shorter path is easier to work with.
const HASH_CHARS: usize = 12;

/// Used when a name reduces to nothing readable, e.g. `"////"`.
const FALLBACK: &str = "x";

/// FNV-1a, 64-bit.
///
/// Deliberately not `std::hash::DefaultHasher`: that is SipHash with a
/// per-process random seed in some configurations and carries no cross-version
/// stability guarantee. These hashes end up in directory names that must still
/// resolve after a Rust upgrade, so the algorithm is pinned here.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Reduces `name` to lowercase alphanumerics separated by single dashes.
fn readable(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_dash = false;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());

            if out.len() >= MAX_READABLE {
                break;
            }
        } else {
            pending_dash = true;
        }
    }

    out
}

/// Builds a filesystem-safe, collision-free component from `name`.
///
/// The result contains only lowercase ASCII alphanumerics and dashes, never
/// starts with a dot, never contains a path separator, and is stable across
/// runs and Rust versions.
///
/// Case matters to the hash even though the readable prefix is lowercased,
/// which is what keeps `Feature` and `feature` distinct on a case-insensitive
/// filesystem like macOS's default APFS.
///
/// ```
/// use ket_core::slug::slug;
///
/// // Different branches never share a slug, however similar they look.
/// assert_ne!(slug("feature/login"), slug("feature-login"));
/// assert_ne!(slug("Feature"), slug("feature"));
/// ```
pub fn slug(name: &str) -> String {
    slug_keyed(name, name.as_bytes())
}

/// Like [`slug`], but the readable prefix and the hashed identity come from
/// different inputs.
///
/// Used where the name a human wants to read is not the thing that has to be
/// unique — a project directory called `api` in two different checkouts should
/// read as `api-…` in both, while remaining distinct.
///
/// ```
/// use ket_core::slug::slug_keyed;
///
/// let a = slug_keyed("api", b"/Users/me/work/api");
/// let b = slug_keyed("api", b"/Users/me/oss/api");
/// assert!(a.starts_with("api-") && b.starts_with("api-"));
/// assert_ne!(a, b);
/// ```
pub fn slug_keyed(readable_from: &str, key: &[u8]) -> String {
    let hash = fnv1a64(key);

    let mut prefix = readable(readable_from);
    if prefix.is_empty() {
        prefix.push_str(FALLBACK);
    }

    let mut out = prefix;
    out.push('-');
    // Low 48 bits, zero-padded, so the suffix is always exactly `HASH_CHARS`.
    let _ = write!(
        out,
        "{:0width$x}",
        hash & 0xffff_ffff_ffff,
        width = HASH_CHARS
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn nested_branch_does_not_become_a_nested_path() {
        let s = slug("feature/login");
        assert!(!s.contains('/'), "slug must be one path component: {s}");
        assert!(!s.contains(std::path::MAIN_SEPARATOR));
    }

    #[test]
    fn nested_branch_does_not_collide_with_its_own_prefix() {
        // The original bug: `feature/login` under a directory named `feature`.
        assert_ne!(slug("feature/login"), slug("feature"));
    }

    #[test]
    fn separators_that_flatten_to_the_same_text_stay_distinct() {
        // Both reduce to the readable form `a-b`; only the hash separates them.
        assert_ne!(slug("a/b"), slug("a-b"));
        assert_ne!(slug("a_b"), slug("a.b"));
    }

    #[test]
    fn case_only_differences_survive_a_case_insensitive_filesystem() {
        // macOS APFS is case-insensitive by default, so these two must not
        // produce names differing only in case.
        let upper = slug("Feature");
        let lower = slug("feature");
        assert_ne!(upper, lower);
        assert_ne!(upper.to_lowercase(), lower.to_lowercase());
    }

    #[test]
    fn dot_names_are_neutralised() {
        for name in [".", "..", ".hidden", "../../etc/passwd"] {
            let s = slug(name);
            assert!(!s.starts_with('.'), "{name} produced {s}");
            assert!(!s.contains(".."), "{name} produced {s}");
            assert!(!s.contains('/'), "{name} produced {s}");
        }
    }

    #[test]
    fn traversal_attempts_cannot_escape_a_directory() {
        let s = slug("../../../etc/passwd");
        assert_eq!(std::path::Path::new(&s).components().count(), 1);
    }

    #[test]
    fn names_with_nothing_readable_still_produce_a_valid_component() {
        for name in ["", "////", "---", "🙂🙂"] {
            let s = slug(name);
            assert!(!s.is_empty());
            assert!(!s.starts_with('-'), "{name} produced {s}");
            assert!(
                s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "{name} produced {s}"
            );
        }
    }

    #[test]
    fn empty_and_unreadable_names_are_still_distinct_from_each_other() {
        assert_ne!(slug(""), slug("////"));
        assert_ne!(slug("🙂"), slug("🙂🙂"));
    }

    #[test]
    fn long_names_are_bounded_well_under_the_filesystem_limit() {
        let long = "a".repeat(5_000);
        let s = slug(&long);
        assert!(
            s.len() <= MAX_READABLE + 1 + HASH_CHARS,
            "len was {}",
            s.len()
        );
        assert!(s.len() < 255);
    }

    #[test]
    fn long_names_sharing_a_prefix_remain_distinct() {
        // Truncation of the readable part must not cause a collision.
        let a = format!("{}-one", "x".repeat(100));
        let b = format!("{}-two", "x".repeat(100));
        assert_ne!(slug(&a), slug(&b));
    }

    #[test]
    fn slugs_are_deterministic() {
        // These names are persisted as directories; instability would orphan
        // every existing worktree.
        assert_eq!(
            slug("729-auto-hide-timestamps"),
            slug("729-auto-hide-timestamps")
        );
        assert_eq!(slug("feature/login"), slug("feature/login"));
    }

    #[test]
    fn hash_suffix_has_a_constant_width() {
        for name in ["a", "", "feature/login", &"z".repeat(200)] {
            let s = slug(name);
            let suffix = s.rsplit('-').next().unwrap();
            assert_eq!(suffix.len(), HASH_CHARS, "{name} produced {s}");
        }
    }

    #[test]
    fn a_realistic_branch_set_produces_no_collisions() {
        let branches = [
            "main",
            "master",
            "feature",
            "feature/login",
            "feature/login-v2",
            "feature-login",
            "Feature/Login",
            "729-auto-hide-timestamps",
            "729-auto-hide-timestamps-2",
            "bugfix/729",
            "bugfix-729",
            "release/1.2.3",
            "release/1.2.30",
            "dependabot/npm_and_yarn/lodash-4.17.21",
        ];

        let slugs: HashSet<String> = branches.iter().map(|b| slug(b)).collect();
        assert_eq!(slugs.len(), branches.len(), "collision among {branches:?}");
    }
}
