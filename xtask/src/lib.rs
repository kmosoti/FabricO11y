//! Small repository integration predicates.
/// A concrete Git-backend commit is a complete forty-digit hexadecimal SHA-1.
/// ```
/// assert!(xtask::valid_commit(&"a".repeat(40)));
/// assert!(!xtask::valid_commit("change-id"));
/// ```
pub fn valid_commit(id: &str) -> bool {
    id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
