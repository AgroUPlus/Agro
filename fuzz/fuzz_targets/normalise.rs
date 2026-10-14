//! Arbitrary tags must never panic the normaliser, and normalising must be stable: a key that
//! changed on a second pass would split one recording in two.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/norm.rs"]
#[allow(dead_code)]
mod norm;

fuzz_target!(|data: (&str, &str)| {
    let (artist, title) = data;
    let key = norm::recording_key(artist, title);
    assert_eq!(norm::normalize_artist(&key.artist), key.artist);
    assert_eq!(norm::normalize_title(&key.title), key.title);
});
