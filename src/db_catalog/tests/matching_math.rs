//! The similarity maths.

use super::*;

#[test]
fn sequence_similarity_is_one_for_identical_and_low_for_unrelated() {
    let a = embedding(10);
    assert!((sequence_similarity(&a, &a) - 1.0).abs() < 1e-6);

    let unrelated = sequence_similarity(&a, &embedding(11));
    assert!(unrelated < MATCH_THRESHOLD, "unrelated scored {unrelated}");
}

/// The claim int8 rests on: a round trip through the wire format still matches the original.
#[test]
fn quantising_to_int8_does_not_cost_a_match() {
    let original = embedding(12);
    let round_tripped = unpack(&blob(&original), DIM);

    let score = sequence_similarity(&original, &round_tripped);
    assert!(
        score >= MATCH_THRESHOLD,
        "int8 round trip scored {score}, below the match threshold"
    );
}
