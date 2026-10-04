//! Who besides its owner may change a playlist, and how much.
//!
//! Opening and editing are separate questions: `playlist_visibility` answers the first, this the
//! second, and the answer here never exceeds the answer there. An owner chooses an [`EditAccess`];
//! what any one account may then do is its [`PlaylistRole`]:
//!
//! - **Editor** — an accepted friend of the owner, when edit access is Friends or Public. Adds,
//!   removes and reorders anything.
//! - **Contributor** — anyone else who can open it, when edit access is Public. Adds, and removes
//!   only what they added themselves: a stranger can grow a public playlist, never take it apart.
//!
//! Renaming, visibility and edit access stay the owner's alone. Defaults closed: edit access is
//! [`EditAccess::Off`] until the owner opens it, and a block on either side means view at most.

use async_graphql::Enum;
use rusqlite::Result;

use crate::db::Db;
use crate::db_playlists::Playlist;
use crate::db_social::FriendState;
use crate::playlist_visibility::PlaylistVisibility;

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum EditAccess {
    Off,
    Friends,
    Public,
}

impl EditAccess {
    /// The `edit_access` column value. Anything unrecognised reads as [`Self::Off`]: a value from a
    /// newer server must not open a playlist wider than this one understands.
    pub fn from_stored(value: i32) -> Self {
        match value {
            1 => Self::Friends,
            2 => Self::Public,
            _ => Self::Off,
        }
    }

    pub fn stored(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Friends => 1,
            Self::Public => 2,
        }
    }

    /// The widest access `visibility` allows: no one may edit what they cannot open, so a playlist
    /// only friends can see cannot take edits from anyone, and a private one from no one.
    pub fn clamped_to(self, visibility: PlaylistVisibility) -> Self {
        match (self, visibility) {
            (_, PlaylistVisibility::Private) => Self::Off,
            (Self::Public, PlaylistVisibility::Friends) => Self::Friends,
            (access, _) => access,
        }
    }
}

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaylistRole {
    Owner,
    Editor,
    Contributor,
    Viewer,
    /// Cannot open it at all.
    None,
}

impl PlaylistRole {
    pub fn can_add(self) -> bool {
        matches!(self, Self::Owner | Self::Editor | Self::Contributor)
    }

    /// Removing or moving any item, not only one's own.
    pub fn can_rearrange(self) -> bool {
        matches!(self, Self::Owner | Self::Editor)
    }
}

impl Db {
    /// What `viewer` may do with `playlist`.
    pub fn playlist_role(&self, playlist: &Playlist, viewer: &str) -> Result<PlaylistRole> {
        // Agro writes a blend; everyone who can open it, its creator included, only reads it.
        if playlist.is_generated() {
            return Ok(if self.can_view_playlist(playlist, viewer)? {
                PlaylistRole::Viewer
            } else {
                PlaylistRole::None
            });
        }
        if playlist.user_id == viewer {
            return Ok(PlaylistRole::Owner);
        }
        if !self.can_view_playlist(playlist, viewer)? {
            return Ok(PlaylistRole::None);
        }
        // Stored values are clamped on every write, but reading through the clamp as well means a
        // row edited by hand still cannot hand out more than its visibility allows.
        let access = playlist.edit_access.clamped_to(playlist.visibility());
        if access == EditAccess::Off {
            return Ok(PlaylistRole::Viewer);
        }
        Ok(match self.friend_state(&playlist.user_id, viewer)? {
            Some(FriendState::Blocked) => PlaylistRole::Viewer,
            Some(FriendState::Accepted) => PlaylistRole::Editor,
            _ if access == EditAccess::Public => PlaylistRole::Contributor,
            _ => PlaylistRole::Viewer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn befriend(db: &Db, a: &str, b: &str) {
        assert!(db.send_friend_request(b, a).unwrap());
        assert!(db.accept_friend_request(a, b).unwrap());
    }

    fn shared(db: &Db, visibility: PlaylistVisibility, access: EditAccess) -> Playlist {
        let pl = db
            .create_playlist("alpha", "Mix", None, visibility)
            .unwrap();
        db.update_playlist_edit_access(&pl.id, "alpha", access)
            .unwrap();
        db.get_playlist(&pl.id).unwrap().unwrap()
    }

    #[test]
    fn the_column_round_trips_and_unknown_values_read_closed() {
        for access in [EditAccess::Off, EditAccess::Friends, EditAccess::Public] {
            assert_eq!(EditAccess::from_stored(access.stored()), access);
        }
        assert_eq!(EditAccess::from_stored(7), EditAccess::Off);
    }

    #[test]
    fn edit_access_never_exceeds_visibility() {
        use PlaylistVisibility::*;
        assert_eq!(EditAccess::Public.clamped_to(Private), EditAccess::Off);
        assert_eq!(EditAccess::Friends.clamped_to(Private), EditAccess::Off);
        assert_eq!(EditAccess::Public.clamped_to(Friends), EditAccess::Friends);
        assert_eq!(EditAccess::Public.clamped_to(Public), EditAccess::Public);
        assert_eq!(EditAccess::Friends.clamped_to(Public), EditAccess::Friends);
    }

    #[test]
    fn a_new_playlist_is_editable_by_its_owner_alone() {
        let db = Db::new_in_memory().unwrap();
        befriend(&db, "alpha", "beta");
        let pl = shared(&db, PlaylistVisibility::Public, EditAccess::Off);

        assert_eq!(db.playlist_role(&pl, "alpha").unwrap(), PlaylistRole::Owner);
        assert_eq!(db.playlist_role(&pl, "beta").unwrap(), PlaylistRole::Viewer);
        assert_eq!(
            db.playlist_role(&pl, "gamma").unwrap(),
            PlaylistRole::Viewer
        );
    }

    #[test]
    fn friends_access_makes_friends_editors_and_nobody_else() {
        let db = Db::new_in_memory().unwrap();
        befriend(&db, "alpha", "beta");
        let pl = shared(&db, PlaylistVisibility::Public, EditAccess::Friends);

        assert_eq!(db.playlist_role(&pl, "beta").unwrap(), PlaylistRole::Editor);
        assert_eq!(
            db.playlist_role(&pl, "gamma").unwrap(),
            PlaylistRole::Viewer
        );
    }

    #[test]
    fn public_access_makes_strangers_contributors_and_friends_still_editors() {
        let db = Db::new_in_memory().unwrap();
        befriend(&db, "alpha", "beta");
        let pl = shared(&db, PlaylistVisibility::Public, EditAccess::Public);

        assert_eq!(db.playlist_role(&pl, "beta").unwrap(), PlaylistRole::Editor);
        assert_eq!(
            db.playlist_role(&pl, "gamma").unwrap(),
            PlaylistRole::Contributor
        );
    }

    #[test]
    fn someone_who_cannot_open_it_has_no_role_at_all() {
        let db = Db::new_in_memory().unwrap();
        let pl = shared(&db, PlaylistVisibility::Friends, EditAccess::Friends);

        assert_eq!(db.playlist_role(&pl, "gamma").unwrap(), PlaylistRole::None);
    }

    #[test]
    fn a_block_on_either_side_means_view_at_most() {
        let db = Db::new_in_memory().unwrap();
        let pl = shared(&db, PlaylistVisibility::Public, EditAccess::Public);

        db.block_user("alpha", "gamma").unwrap();
        assert_eq!(
            db.playlist_role(&pl, "gamma").unwrap(),
            PlaylistRole::Viewer
        );

        db.block_user("delta", "alpha").unwrap();
        assert_eq!(
            db.playlist_role(&pl, "delta").unwrap(),
            PlaylistRole::Viewer
        );
    }

    #[test]
    fn narrowing_visibility_narrows_edit_access_with_it() {
        let db = Db::new_in_memory().unwrap();
        let pl = shared(&db, PlaylistVisibility::Public, EditAccess::Public);

        db.update_playlist_visibility(&pl.id, "alpha", PlaylistVisibility::Friends)
            .unwrap();
        let friends = db.get_playlist(&pl.id).unwrap().unwrap();
        assert_eq!(friends.edit_access, EditAccess::Friends);

        db.update_playlist_visibility(&pl.id, "alpha", PlaylistVisibility::Private)
            .unwrap();
        let private = db.get_playlist(&pl.id).unwrap().unwrap();
        assert_eq!(private.edit_access, EditAccess::Off);

        // Widening it again does not quietly reopen editing: that stays the owner's choice.
        db.update_playlist_visibility(&pl.id, "alpha", PlaylistVisibility::Public)
            .unwrap();
        let reopened = db.get_playlist(&pl.id).unwrap().unwrap();
        assert_eq!(reopened.edit_access, EditAccess::Off);
    }

    #[test]
    fn only_the_owner_can_change_edit_access() {
        let db = Db::new_in_memory().unwrap();
        let pl = shared(&db, PlaylistVisibility::Public, EditAccess::Off);

        assert_eq!(
            db.update_playlist_edit_access(&pl.id, "beta", EditAccess::Public)
                .unwrap(),
            None
        );
        assert_eq!(
            db.get_playlist(&pl.id).unwrap().unwrap().edit_access,
            EditAccess::Off
        );
    }
}
