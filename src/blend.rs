//! Choosing a Blend's tracks from its members' listening.
//!
//! Pure: [`generate`] takes each member's most-played tracks and a recipe and returns the picks.
//! Reading the history and writing the playlist are `db_blend`'s, so the rules here are tested
//! without a database.
//!
//! Two pools. **Common ground** is what members share: a track two or more of them play, ranked
//! by how many share it and how high it sits for each; then tracks by an artist two or more of them
//! play. **Discovery** is each member's own favourites the others have not played, taken in turns
//! so everyone gets the same share. `mix` sets the split, and either pool tops up the other when it
//! runs dry, so a blend of two people with nothing in common is still full.

use std::collections::{HashMap, HashSet};

use crate::norm::{normalize_artist, recording_key};

/// One track as a member has played it, most-played first in [`MemberTaste::tracks`].
#[derive(Clone, Debug, PartialEq)]
pub struct TasteTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub plays: i64,
}

#[derive(Clone, Debug)]
pub struct MemberTaste {
    pub username: String,
    pub tracks: Vec<TasteTrack>,
}

#[derive(Clone, Copy, Debug)]
pub struct Recipe {
    pub size: usize,
    /// 0 is all common ground, 100 all discovery.
    pub mix: u8,
    /// Fixes the order, so one refresh always reads the same and the next one differs.
    pub seed: u64,
}

/// A chosen track, and whose listening put it there — first the one who plays it most.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub track: TasteTrack,
    pub from: Vec<String>,
}

struct Candidate {
    track: TasteTrack,
    artist: String,
    /// (member index, that member's rank for it), best rank first.
    lovers: Vec<(usize, usize)>,
}

pub fn generate(members: &[MemberTaste], recipe: Recipe) -> Vec<Pick> {
    let mut by_key: HashMap<(String, String, String), usize> = HashMap::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for (m, member) in members.iter().enumerate() {
        for (rank, track) in member.tracks.iter().enumerate() {
            let key = recording_key(&track.artist, &track.title);
            let idx = *by_key
                .entry((key.artist, key.title, key.variants))
                .or_insert_with(|| {
                    candidates.push(Candidate {
                        track: track.clone(),
                        artist: normalize_artist(&track.artist),
                        lovers: Vec::new(),
                    });
                    candidates.len() - 1
                });
            let lovers = &mut candidates[idx].lovers;
            if !lovers.iter().any(|(who, _)| *who == m) {
                lovers.push((m, rank));
            }
        }
    }
    for c in &mut candidates {
        c.lovers.sort_by_key(|(_, rank)| *rank);
    }

    let mut artist_fans: HashMap<&str, HashSet<usize>> = HashMap::new();
    for c in &candidates {
        let fans = artist_fans.entry(c.artist.as_str()).or_default();
        fans.extend(c.lovers.iter().map(|(m, _)| *m));
    }
    let shared_artist = |c: &Candidate| {
        artist_fans
            .get(c.artist.as_str())
            .is_some_and(|f| f.len() >= 2)
    };

    // Shared tracks first: by how many share them, then by how high they sit for those who do.
    let mut shared: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].lovers.len() >= 2)
        .collect();
    let score = |i: usize| -> f64 {
        candidates[i]
            .lovers
            .iter()
            .map(|(_, r)| 1.0 / (1 + r) as f64)
            .sum()
    };
    shared.sort_by(|&a, &b| {
        (candidates[b].lovers.len(), score(b))
            .partial_cmp(&(candidates[a].lovers.len(), score(a)))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    // Then one member's track by an artist another member also plays, and then their own picks.
    let solo = |want_shared_artist: bool| -> Vec<Vec<usize>> {
        let mut lists = vec![Vec::new(); members.len()];
        for (i, c) in candidates.iter().enumerate() {
            if c.lovers.len() == 1 && shared_artist(c) == want_shared_artist {
                lists[c.lovers[0].0].push((c.lovers[0].1, i));
            }
        }
        lists
            .into_iter()
            .map(|mut l| {
                l.sort();
                l.into_iter().map(|(_, i)| i).collect()
            })
            .collect()
    };
    let common: Vec<usize> = shared.into_iter().chain(round_robin(solo(true))).collect();
    let discovery: Vec<usize> = round_robin(solo(false));

    let common_target = (recipe.size * (100 - recipe.mix.min(100) as usize) + 50) / 100;
    let artist_cap = (recipe.size / 8).max(2);
    let mut taken: Vec<usize> = Vec::new();
    let mut per_artist: HashMap<&str, usize> = HashMap::new();
    let mut take = |pool: &[usize], until: usize, taken: &mut Vec<usize>| {
        for &i in pool {
            if taken.len() >= until {
                break;
            }
            let count = per_artist.entry(candidates[i].artist.as_str()).or_default();
            if *count < artist_cap && !taken.contains(&i) {
                *count += 1;
                taken.push(i);
            }
        }
    };
    take(&common, common_target, &mut taken);
    take(&discovery, recipe.size, &mut taken);
    take(&common, recipe.size, &mut taken);

    shuffle(&mut taken, recipe.seed);
    taken
        .into_iter()
        .map(|i| Pick {
            track: candidates[i].track.clone(),
            from: candidates[i]
                .lovers
                .iter()
                .map(|(m, _)| members[*m].username.clone())
                .collect(),
        })
        .collect()
}

/// One from each list in turn until all are empty: fairness between members.
fn round_robin(lists: Vec<Vec<usize>>) -> Vec<usize> {
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    (0..longest)
        .flat_map(|n| lists.iter().filter_map(move |l| l.get(n).copied()))
        .collect()
}

/// Fisher–Yates on SplitMix64: deterministic, so a refresh can be reproduced from its seed.
fn shuffle(items: &mut [usize], seed: u64) {
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..items.len()).rev() {
        items.swap(i, (next() % (i as u64 + 1)) as usize);
    }
}
