import { useEffect, useState } from 'react';
import { ArrowLeft, Disc3 } from 'lucide-react';
import { coverUrl, getAlbum } from '../playlistApi.js';
import DetailHero from '../components/library/DetailHero.jsx';
import TrackTable from '../components/library/TrackTable.jsx';
import { formatTotal, songCount } from '../components/library/format.js';

/** An album in the library, track by track. Read-only: an album is what it is. */
export default function AlbumDetail({ id, username, onBack, onUnauthorized }) {
  const [album, setAlbum] = useState(undefined);
  const [error, setError] = useState('');

  useEffect(() => {
    let live = true;
    getAlbum(username, id)
      .then((found) => live && setAlbum(found))
      .catch((failure) => {
        if (failure.unauthorized) onUnauthorized?.();
        else if (live) setError(failure.message);
      });
    return () => {
      live = false;
    };
  }, [username, id, onUnauthorized]);

  const back = (
    <button type="button" className="pill-btn detail-back" onClick={onBack}>
      <ArrowLeft size={14} /> Library
    </button>
  );

  if (!album) {
    return (
      <div>
        {back}
        <div className="empty-hint">
          {error || (album === null ? 'This album is no longer in your library.' : 'Loading…')}
        </div>
      </div>
    );
  }

  const multiDisc = new Set(album.tracks.map((t) => t.discNo ?? 1)).size > 1;
  const rows = album.tracks.map((track, index) => ({
    key: track.id,
    number: track.trackNo ? (multiDisc ? `${track.discNo ?? 1}.${track.trackNo}` : track.trackNo) : index + 1,
    title: track.title,
    artist: track.artist,
    durationMs: track.durationMs
  }));

  return (
    <div>
      {back}
      <DetailHero
        cover={album.coverKey ? <img src={coverUrl(album.coverKey)} alt="" /> : <Disc3 size={56} />}
        kind="Album"
        title={album.title}
        byline={
          <>
            <strong>{album.artist}</strong>
            {album.year && <span>· {album.year}</span>}
            <span>· {songCount(album.tracks.length)}, {formatTotal(album.totalDurationMs)}</span>
          </>
        }
      />
      <div className="detail-actions" />
      <TrackTable rows={rows} columns={{ art: false }} />
    </div>
  );
}
