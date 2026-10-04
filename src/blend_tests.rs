//! How a Blend chooses: shared taste first, everyone's share of the rest, and no one artist
//! running away with it.
#![cfg(test)]

use crate::blend::{generate, MemberTaste, Recipe, TasteTrack};

fn t(artist: &str, title: &str) -> TasteTrack {
    TasteTrack {
        title: title.into(),
        artist: artist.into(),
        album: None,
        duration_ms: Some(200_000),
        plays: 1,
    }
}

fn member(name: &str, tracks: Vec<TasteTrack>) -> MemberTaste {
    MemberTaste {
        username: name.into(),
        tracks,
    }
}

/// `n` tracks by `n` different artists, so the per-artist cap never decides anything.
fn own(name: &str, n: usize) -> Vec<TasteTrack> {
    (0..n)
        .map(|i| t(&format!("{name} artist {i}"), &format!("{name} song {i}")))
        .collect()
}

fn recipe(size: usize, mix: u8) -> Recipe {
    Recipe { size, mix, seed: 7 }
}

#[test]
fn a_track_two_members_share_is_common_ground_and_credits_both() {
    let mut alex = own("alex", 10);
    let mut sam = own("sam", 10);
    alex.insert(0, t("Shared Band", "Both Love This"));
    sam.push(t("shared band", "Both Love This (feat. Someone)"));
    let picks = generate(&[member("alex", alex), member("sam", sam)], recipe(1, 0));
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0].track.title, "Both Love This");
    assert_eq!(
        picks[0].from,
        ["alex", "sam"],
        "the higher-ranked fan comes first"
    );
}

#[test]
fn discovery_gives_each_member_an_equal_share() {
    let members = [
        member("alex", own("alex", 30)),
        member("sam", own("sam", 30)),
        member("kim", own("kim", 30)),
    ];
    let picks = generate(&members, recipe(30, 100));
    for name in ["alex", "sam", "kim"] {
        let mine = picks.iter().filter(|p| p.from[0] == name).count();
        assert_eq!(mine, 10, "{name} got {mine} of 30");
    }
}

#[test]
fn a_dry_pool_is_topped_up_from_the_other_so_the_blend_is_full() {
    // Nothing in common at all, yet asked for all common ground.
    let members = [
        member("alex", own("alex", 20)),
        member("sam", own("sam", 20)),
    ];
    assert_eq!(generate(&members, recipe(25, 0)).len(), 25);
}

#[test]
fn it_never_holds_more_than_there_is() {
    let members = [member("alex", own("alex", 3)), member("sam", own("sam", 2))];
    assert_eq!(generate(&members, recipe(50, 60)).len(), 5);
}

#[test]
fn one_artist_cannot_take_over() {
    let flood: Vec<_> = (0..40)
        .map(|i| t("Same Artist", &format!("hit {i}")))
        .collect();
    let members = [member("alex", flood), member("sam", own("sam", 40))];
    let picks = generate(&members, recipe(40, 100));
    let same = picks
        .iter()
        .filter(|p| p.track.artist == "Same Artist")
        .count();
    assert_eq!(same, 5, "the cap is a size/8 share per artist");
}

#[test]
fn shared_artists_count_as_common_ground_before_anyone_s_solo_picks() {
    let alex = vec![
        t("Band", "Alex's song by Band"),
        t("Elsewhere", "alex only"),
    ];
    let sam = vec![t("Band", "Sam's song by Band"), t("Nowhere", "sam only")];
    let picks = generate(&[member("alex", alex), member("sam", sam)], recipe(2, 0));
    assert!(picks.iter().all(|p| p.track.artist == "Band"), "{picks:?}");
}

#[test]
fn the_same_seed_gives_the_same_order_and_another_does_not() {
    let members = [
        member("alex", own("alex", 30)),
        member("sam", own("sam", 30)),
    ];
    let a = generate(&members, recipe(30, 50));
    let b = generate(&members, recipe(30, 50));
    let c = generate(
        &members,
        Recipe {
            seed: 8,
            ..recipe(30, 50)
        },
    );
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn a_member_with_no_history_contributes_nothing_and_breaks_nothing() {
    let members = [member("alex", own("alex", 10)), member("new", vec![])];
    let picks = generate(&members, recipe(10, 50));
    assert_eq!(picks.len(), 10);
    assert!(picks.iter().all(|p| p.from == ["alex"]));
}
