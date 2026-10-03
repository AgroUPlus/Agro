import { useCallback, useEffect, useRef, useState } from 'react';
import { applyEdits, getPlaylist, playlistRevision, updateDetails } from '../playlistApi.js';
import { replayEdits } from '../playlistReplay.js';

/** How often an open playlist checks whether someone else has changed it. */
const POLL_MS = 6000;

const isStale = (failure) => failure?.code === 'STALE_REVISION';

/**
 * One playlist, kept in step with the server.
 *
 * `status` is what the sync chip shows: `loading`, `synced`, `saving`, `stale` (someone else got
 * there first; refreshing), `error` (the last write failed) or `revoked` (gone, or no longer shared
 * with us). Every write names the revision it was made against; a stale one is refetched, the
 * parts that still make sense are replayed once, and `banner` says what happened.
 */
export default function usePlaylistSync(id, onUnauthorized) {
  const [playlist, setPlaylist] = useState(null);
  const [status, setStatus] = useState('loading');
  const [banner, setBanner] = useState('');
  const current = useRef(null);
  const busy = useRef(false);

  const adopt = useCallback((next) => {
    current.current = next;
    setPlaylist(next);
  }, []);

  const fail = useCallback(
    (failure) => {
      if (failure.unauthorized) onUnauthorized?.();
      setStatus('error');
      setBanner(failure.message);
    },
    [onUnauthorized]
  );

  const load = useCallback(async () => {
    try {
      adopt(await getPlaylist(id));
      setStatus('synced');
    } catch (failure) {
      if (failure.unauthorized) onUnauthorized?.();
      // Missing and forbidden read the same to us, as the server means them to.
      setStatus('revoked');
    }
  }, [id, adopt, onUnauthorized]);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    const timer = setInterval(async () => {
      if (busy.current || !current.current || document.hidden) return;
      try {
        const revision = await playlistRevision(id);
        if (revision === null) setStatus('revoked');
        else if (revision !== current.current.revision) {
          await load();
          setBanner('Updated by someone else — refreshed.');
        }
      } catch (failure) {
        if (failure.unauthorized) onUnauthorized?.();
      }
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [id, load, onUnauthorized]);

  /** Sends `edits`, replaying them once against a fresh copy if the playlist moved on meanwhile. */
  const commit = useCallback(
    async (edits) => {
      if (!current.current || busy.current) return;
      busy.current = true;
      setStatus('saving');
      try {
        adopt(await applyEdits(id, current.current.revision, edits));
        setStatus('synced');
      } catch (failure) {
        if (!isStale(failure)) {
          fail(failure);
          return;
        }
        setStatus('stale');
        try {
          const fresh = await getPlaylist(id);
          const { kept, dropped } = replayEdits(edits, fresh);
          adopt(kept.length ? await applyEdits(id, fresh.revision, kept) : fresh);
          setStatus('synced');
          setBanner(
            dropped
              ? `Someone else changed this playlist first; ${dropped} of your changes no longer applied.`
              : 'Someone else changed this playlist first; your change was applied on top.'
          );
        } catch (retry) {
          if (isStale(retry)) {
            await load();
            setBanner('This playlist is changing fast — it has been refreshed; try again.');
          } else fail(retry);
        }
      } finally {
        busy.current = false;
      }
    },
    [id, adopt, fail, load]
  );

  const saveDetails = useCallback(
    async (title, description) => {
      if (!current.current) return false;
      try {
        adopt(await updateDetails(id, current.current.revision, title, description));
        return true;
      } catch (failure) {
        if (!isStale(failure)) {
          fail(failure);
          return false;
        }
        await load();
        setBanner('Someone changed this playlist while you were editing — check it and save again.');
        return false;
      }
    },
    [id, adopt, fail, load]
  );

  /** Runs an owner setting change, then reloads to pick up what the server actually stored. */
  const change = useCallback(
    async (action) => {
      try {
        await action();
        await load();
      } catch (failure) {
        fail(failure);
      }
    },
    [load, fail]
  );

  return { playlist, status, banner, dismissBanner: () => setBanner(''), commit, saveDetails, change, reload: load };
}
