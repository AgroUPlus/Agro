use super::*;

#[test]
fn both_in_range_are_carried() {
    assert_eq!(playback_suffix(Some(1.25), Some(0.9)), "&s=1.25&p=0.9");
}

#[test]
fn one_without_the_other_is_dropped() {
    assert_eq!(playback_suffix(Some(1.25), None), "");
    assert_eq!(playback_suffix(None, Some(0.9)), "");
}

#[test]
fn neither_is_the_ordinary_case() {
    assert_eq!(playback_suffix(None, None), "");
}

/// Dropped, not clamped. Clamping would invent an intent the link does not carry.
#[test]
fn out_of_range_is_dropped_not_clamped() {
    assert_eq!(playback_suffix(Some(40.0), Some(1.0)), "");
    assert_eq!(playback_suffix(Some(1.0), Some(0.1)), "");
}

#[test]
fn the_bounds_are_inclusive() {
    assert_eq!(playback_suffix(Some(0.5), Some(2.0)), "&s=0.5&p=2");
}
