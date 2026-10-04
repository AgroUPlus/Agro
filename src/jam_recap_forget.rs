//! Removing a deleted account from the recaps other people kept.
//!
//! A recap belongs to its reader, so it survives when someone else in the room deletes their
//! account — but their name does not. Deleting an account removes every mention of it, and a recap
//! names people twice over: in who was there, and in who queued each track.

use crate::jam_recap::{top_contributor, JamRecap, RecapTrack};

impl JamRecap {
    /// Strikes `username` from this recap. Returns whether anything changed.
    ///
    /// Their tracks stay — the room heard them, and the reader's memory of the night is the
    /// reader's — but nothing says who chose them. "Top DJ" is worked out again from whoever is
    /// still named, so a deleted account cannot linger as the answer to it.
    pub fn forget(&mut self, username: &str) -> bool {
        let name = username.trim().to_lowercase();
        let before = self.people.len();
        self.people.retain(|p| *p != name);
        let mut changed = self.people.len() != before;

        let tracks = self
            .tracks
            .iter_mut()
            .chain(self.most_loved.iter_mut())
            .chain(self.most_skipped.iter_mut());
        for track in tracks {
            changed |= unname(track, &name);
        }
        if changed {
            self.top_contributor = top_contributor(&self.tracks);
        }
        changed
    }
}

fn unname(track: &mut RecapTrack, name: &str) -> bool {
    if track.added_by.eq_ignore_ascii_case(name) {
        track.added_by.clear();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::jam_recap::{JamRecap, RecapTrack};

    fn track(title: &str, by: &str, approvals: i64) -> RecapTrack {
        RecapTrack {
            title: title.into(),
            artist: "Artist".into(),
            artwork_url: None,
            track_uri: format!("uri:{title}"),
            added_by: by.into(),
            duration_ms: 1000,
            approvals,
            skip_votes: 0,
        }
    }

    fn recap() -> JamRecap {
        let played = vec![
            track("a", "sam", 2),
            track("b", "sam", 0),
            track("c", "alex", 0),
        ];
        let people = ["alex".to_string(), "kim".to_string()];
        JamRecap::build(
            "2026-10-04T20:00:00+00:00",
            "2026-10-04T21:00:00+00:00",
            &people,
            played,
        )
        .unwrap()
    }

    #[test]
    fn a_forgotten_member_is_named_nowhere_but_their_tracks_stay() {
        let mut recap = recap();
        assert_eq!(recap.top_contributor.as_ref().unwrap().username, "sam");

        assert!(recap.forget("Sam"));

        let json = serde_json::to_string(&recap).unwrap();
        assert!(!json.contains("\"sam\""), "still named: {json}");
        assert_eq!(recap.tracks.len(), 3);
        assert_eq!(recap.people, ["alex", "kim"]);
        assert_eq!(recap.most_loved.as_ref().unwrap().added_by, "");
        assert_eq!(recap.top_contributor.unwrap().username, "alex");
    }

    #[test]
    fn forgetting_someone_who_was_not_there_changes_nothing() {
        let mut recap = recap();
        let before = recap.clone();
        assert!(!recap.forget("nobody"));
        assert_eq!(recap, before);
    }
}
