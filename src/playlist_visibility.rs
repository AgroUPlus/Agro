//! Who can open a playlist.
//!
//! Three levels, stored in two columns so the one that already existed keeps its meaning:
//! `is_public` still answers "can every signed-in account open this", and `friends_only` adds the
//! step between that and owner-only. Every existing row has `friends_only = 0`, so a playlist that
//! was private stays private and one that was public stays public.
//!
//! Defaults closed: a playlist is [`PlaylistVisibility::Private`] unless someone chose otherwise.

use async_graphql::Enum;
use rusqlite::Result;

use crate::db::Db;
use crate::db_playlists::Playlist;

/// Private is the owner alone. Friends adds the owner's accepted friends. Public is every
/// signed-in account on this server.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaylistVisibility {
    Private,
    Friends,
    Public,
}

impl PlaylistVisibility {
    /// The `(is_public, friends_only)` columns that store this level. `friends_only` is left
    /// unset for a public playlist: it means nothing once everyone can open it.
    pub fn flags(self) -> (bool, bool) {
        match self {
            Self::Private => (false, false),
            Self::Friends => (false, true),
            Self::Public => (true, false),
        }
    }

    pub fn from_flags(is_public: bool, friends_only: bool) -> Self {
        if is_public {
            Self::Public
        } else if friends_only {
            Self::Friends
        } else {
            Self::Private
        }
    }

    /// What the original `isPublic` boolean asked for, for clients that predate the three levels.
    pub fn from_public_flag(is_public: bool) -> Self {
        Self::from_flags(is_public, false)
    }
}

impl Playlist {
    pub fn visibility(&self) -> PlaylistVisibility {
        PlaylistVisibility::from_flags(self.is_public, self.friends_only)
    }
}

impl Db {
    /// Whether `viewer` may open `playlist`. The owner always may. A friends-only playlist is open
    /// to the owner's *accepted* friends: a pending request, a stranger, and anyone either side
    /// has blocked all get the same answer, no.
    ///
    /// A generated playlist ignores its visibility: a blend is open to whoever has joined it and
    /// nobody else, because what it holds is its members' listening.
    pub fn can_view_playlist(&self, playlist: &Playlist, viewer: &str) -> Result<bool> {
        if playlist.user_id == viewer {
            return Ok(true);
        }
        if playlist.is_generated() {
            return self.is_blend_member(&playlist.id, viewer);
        }
        match playlist.visibility() {
            PlaylistVisibility::Public => Ok(true),
            PlaylistVisibility::Friends => self.are_friends(&playlist.user_id, viewer),
            PlaylistVisibility::Private => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(db: &Db, owner: &str, visibility: PlaylistVisibility) -> Playlist {
        db.create_playlist(owner, "Mix", None, visibility).unwrap()
    }

    fn befriend(db: &Db, a: &str, b: &str) {
        assert!(db.send_friend_request(b, a).unwrap());
        assert!(db.accept_friend_request(a, b).unwrap());
    }

    #[test]
    fn the_two_columns_round_trip_every_level() {
        for level in [
            PlaylistVisibility::Private,
            PlaylistVisibility::Friends,
            PlaylistVisibility::Public,
        ] {
            let (is_public, friends_only) = level.flags();
            assert_eq!(
                PlaylistVisibility::from_flags(is_public, friends_only),
                level
            );
        }
    }

    #[test]
    fn the_original_boolean_still_means_public_or_private() {
        assert_eq!(
            PlaylistVisibility::from_public_flag(true),
            PlaylistVisibility::Public
        );
        assert_eq!(
            PlaylistVisibility::from_public_flag(false),
            PlaylistVisibility::Private
        );
    }

    #[test]
    fn a_playlist_is_stored_and_read_back_at_the_level_it_was_made() {
        let db = Db::new_in_memory().unwrap();
        for level in [
            PlaylistVisibility::Private,
            PlaylistVisibility::Friends,
            PlaylistVisibility::Public,
        ] {
            let made = playlist(&db, "alpha", level);
            assert_eq!(made.visibility(), level);
            assert_eq!(
                db.get_playlist(&made.id).unwrap().unwrap().visibility(),
                level
            );
        }
    }

    #[test]
    fn private_is_the_owner_alone_even_for_a_friend() {
        let db = Db::new_in_memory().unwrap();
        befriend(&db, "alpha", "beta");
        let pl = playlist(&db, "alpha", PlaylistVisibility::Private);

        assert!(db.can_view_playlist(&pl, "alpha").unwrap());
        assert!(!db.can_view_playlist(&pl, "beta").unwrap());
        assert!(!db.can_view_playlist(&pl, "gamma").unwrap());
    }

    #[test]
    fn friends_only_reaches_accepted_friends_and_nobody_else() {
        let db = Db::new_in_memory().unwrap();
        let pl = playlist(&db, "alpha", PlaylistVisibility::Friends);
        assert!(!db.can_view_playlist(&pl, "beta").unwrap(), "a stranger");

        db.send_friend_request("beta", "alpha").unwrap();
        assert!(
            !db.can_view_playlist(&pl, "beta").unwrap(),
            "a request not yet accepted"
        );

        db.accept_friend_request("alpha", "beta").unwrap();
        assert!(
            db.can_view_playlist(&pl, "beta").unwrap(),
            "an accepted friend"
        );
        assert!(db.can_view_playlist(&pl, "alpha").unwrap(), "the owner");
        assert!(!db.can_view_playlist(&pl, "gamma").unwrap(), "someone else");

        db.remove_friend("alpha", "beta").unwrap();
        assert!(
            !db.can_view_playlist(&pl, "beta").unwrap(),
            "after unfriending"
        );
    }

    #[test]
    fn public_is_open_to_any_account() {
        let db = Db::new_in_memory().unwrap();
        let pl = playlist(&db, "alpha", PlaylistVisibility::Public);

        assert!(db.can_view_playlist(&pl, "gamma").unwrap());
    }

    #[test]
    fn changing_the_level_changes_who_can_open_it() {
        let db = Db::new_in_memory().unwrap();
        befriend(&db, "alpha", "beta");
        let pl = playlist(&db, "alpha", PlaylistVisibility::Private);

        assert!(db
            .update_playlist_visibility(&pl.id, "alpha", PlaylistVisibility::Friends)
            .unwrap());
        let now = db.get_playlist(&pl.id).unwrap().unwrap();
        assert!(db.can_view_playlist(&now, "beta").unwrap());

        assert!(db
            .update_playlist_visibility(&pl.id, "alpha", PlaylistVisibility::Private)
            .unwrap());
        let back = db.get_playlist(&pl.id).unwrap().unwrap();
        assert!(!db.can_view_playlist(&back, "beta").unwrap());
    }

    #[test]
    fn only_the_owner_can_change_the_level() {
        let db = Db::new_in_memory().unwrap();
        let pl = playlist(&db, "alpha", PlaylistVisibility::Private);

        assert!(!db
            .update_playlist_visibility(&pl.id, "beta", PlaylistVisibility::Public)
            .unwrap());
        assert_eq!(
            db.get_playlist(&pl.id).unwrap().unwrap().visibility(),
            PlaylistVisibility::Private
        );
    }

    #[test]
    fn the_friends_listing_holds_friends_only_playlists_and_nothing_else() {
        let db = Db::new_in_memory().unwrap();
        playlist(&db, "alpha", PlaylistVisibility::Private);
        playlist(&db, "alpha", PlaylistVisibility::Public);
        let shared = playlist(&db, "alpha", PlaylistVisibility::Friends);
        playlist(&db, "beta", PlaylistVisibility::Friends);

        let listed = db.list_friends_only_playlists("alpha").unwrap();

        assert_eq!(
            listed.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            vec![shared.id]
        );
    }
}
