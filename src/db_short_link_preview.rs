//! What a short link is *of*: the title and subtitle a chat's link preview shows.
//!
//! A third `impl Db` block for short links, beside [`crate::db_short_links`]. The sharer's app
//! sends these with the link; `/listen` puts them in its Open Graph tags (see `listen::preview`).
//! They are library metadata the sharer chose to put on a public link — never anything about
//! whoever opens it.

use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;

/// Longest title or subtitle kept. A preview line is cut well before this by every chat client;
/// the bound is so a stranger with an account cannot park a novel behind a link.
pub const MAX_PREVIEW_CHARS: usize = 200;

/// What a link shows in a preview. `title` is always present when a preview is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPreview {
    pub title: String,
    pub subtitle: Option<String>,
}

impl Db {
    /// Records what short link `id` is of.
    ///
    /// A value left out keeps what was stored, so re-sharing a link from a client that sends
    /// nothing never blanks a preview it already had.
    pub fn set_short_link_preview(
        &self,
        id: &str,
        title: Option<&str>,
        subtitle: Option<&str>,
    ) -> Result<()> {
        if title.is_none() && subtitle.is_none() {
            return Ok(());
        }
        let conn = self.conn.lock_recover();
        conn.execute(
            "UPDATE short_links SET title = COALESCE(?2, title), subtitle = COALESCE(?3, subtitle)
             WHERE id = ?1",
            params![id, title, subtitle],
        )?;
        Ok(())
    }

    /// The preview for short link `id`, or `None` when it has no title to show.
    pub fn short_link_preview(&self, id: &str) -> Result<Option<LinkPreview>> {
        let conn = self.read();
        let row: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT title, subtitle FROM short_links WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(title, subtitle)| title.map(|title| LinkPreview { title, subtitle })))
    }
}

/// Text fit to store as a preview: trimmed, without control characters, at most
/// [`MAX_PREVIEW_CHARS`] long, and `None` when nothing is left.
pub fn clean_preview_text(raw: Option<&str>) -> Option<String> {
    let cleaned: String = raw?
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_PREVIEW_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_link() -> Db {
        let db = Db::new(":memory:").unwrap();
        db.create_short_link(
            "abc1234",
            "https://music.youtube.com/watch?v=x",
            None,
            None,
            None,
        )
        .unwrap();
        db
    }

    #[test]
    fn a_link_without_metadata_has_no_preview() {
        assert_eq!(db_with_link().short_link_preview("abc1234").unwrap(), None);
    }

    #[test]
    fn stored_metadata_comes_back() {
        let db = db_with_link();
        db.set_short_link_preview("abc1234", Some("Paranoid Android"), Some("Radiohead"))
            .unwrap();
        assert_eq!(
            db.short_link_preview("abc1234").unwrap(),
            Some(LinkPreview {
                title: "Paranoid Android".into(),
                subtitle: Some("Radiohead".into())
            })
        );
    }

    #[test]
    fn sharing_again_without_metadata_keeps_it() {
        let db = db_with_link();
        db.set_short_link_preview("abc1234", Some("Airbag"), None)
            .unwrap();
        db.set_short_link_preview("abc1234", None, None).unwrap();
        let preview = db.short_link_preview("abc1234").unwrap().unwrap();
        assert_eq!(preview.title, "Airbag");
    }

    #[test]
    fn cleaning_trims_strips_and_bounds() {
        assert_eq!(
            clean_preview_text(Some("  Airbag\n ")),
            Some("Airbag".into())
        );
        assert_eq!(clean_preview_text(Some(" \t ")), None);
        assert_eq!(clean_preview_text(None), None);
        let long = "x".repeat(MAX_PREVIEW_CHARS + 50);
        assert_eq!(
            clean_preview_text(Some(&long)).unwrap().chars().count(),
            MAX_PREVIEW_CHARS
        );
    }
}
