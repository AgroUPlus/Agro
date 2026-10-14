use super::*;

#[test]
pub(super) fn only_lowercase_hex_of_the_right_length_is_a_hash() {
    assert!(is_sha256_hex(&"a".repeat(64)));
    assert!(!is_sha256_hex(&"A".repeat(64)), "uppercase is rejected");
    assert!(!is_sha256_hex(&"a".repeat(63)));
    assert!(!is_sha256_hex(&"g".repeat(64)));
    assert!(!is_sha256_hex("../../etc/passwd"));
    assert!(!is_sha256_hex(""));
}
