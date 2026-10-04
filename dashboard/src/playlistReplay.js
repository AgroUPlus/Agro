/**
 * What is left of an edit once someone else has changed the playlist first.
 *
 * An add always survives: a track someone wanted in the playlist still belongs there. If the
 * track it was meant to follow has gone, it goes at the end instead. A remove or a move only makes
 * sense while its item is still there; one whose item has gone is dropped, and so is a move whose
 * anchor has gone, since where it was meant to land no longer exists. Dropped edits are counted so
 * the page can say so rather than pretend they happened.
 *
 * Mirrors the replay in Wanda's `SharedPlaylistSync`.
 */
export function replayEdits(edits, fresh) {
  const present = new Set(fresh.items.map((item) => item.id));
  const kept = [];
  let dropped = 0;

  for (const edit of edits) {
    if (edit.add) {
      const anchor = edit.add.afterItemId;
      kept.push({ add: { ...edit.add, afterItemId: anchor && present.has(anchor) ? anchor : null } });
    } else if (edit.remove) {
      if (present.has(edit.remove)) kept.push(edit);
      else dropped += 1;
    } else if (edit.move) {
      const { itemId, afterItemId } = edit.move;
      if (present.has(itemId) && (!afterItemId || present.has(afterItemId))) kept.push(edit);
      else dropped += 1;
    }
  }
  return { kept, dropped };
}
