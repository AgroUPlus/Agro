import { useState } from 'react';
import { ArrowDown, ArrowUp, Music2 } from 'lucide-react';
import { shapeMask } from '../../expressive/shapes.js';

const PODIUM_SHAPES = ['cookie', 'clover', 'sunny'];

/** A cover, or the same note icon the site's chart shows when there is none or it will not load. */
function Cover({ url, shape }) {
  const [broken, setBroken] = useState(false);
  const mask = shape ? shapeMask(shape) : undefined;
  return (
    <span className={`chart-cover ${shape ? 'chart-cover-shaped' : ''}`} style={mask && { mask, WebkitMask: mask }}>
      {url && !broken ? <img src={url} alt="" loading="lazy" onError={() => setBroken(true)} /> : <Music2 size={20} />}
    </span>
  );
}

/**
 * Movement against the window before, only when the server says that comparison means something:
 * otherwise every row would read "new".
 */
function Movement({ track, rank, compared }) {
  if (!compared) return null;
  if (track.previous_rank == null) return <span className="move move-new">New</span>;
  const delta = track.previous_rank - rank;
  if (delta === 0) return <span className="move move-same">–</span>;
  return delta > 0
    ? <span className="move move-up" title={`Up from #${track.previous_rank}`}><ArrowUp size={13} />{delta}</span>
    : <span className="move move-down" title={`Down from #${track.previous_rank}`}><ArrowDown size={13} />{-delta}</span>;
}

/** The top three as covers on a podium, each cut to a different Material 3 Expressive shape. */
export function PopularPodium({ tracks, compared }) {
  return (
    <div className="podium">
      {tracks.map((track, index) => (
        <div key={`${track.artist}-${track.title}`} className={`podium-spot podium-${index + 1}`}>
          <div className="podium-art">
            <Cover url={track.cover_url} shape={PODIUM_SHAPES[index]} />
            <span className="podium-rank">{index + 1}</span>
          </div>
          <div className="podium-title" title={track.title}>{track.title}</div>
          <div className="podium-artist">{track.artist}</div>
          <div className="podium-meta">
            <span className="podium-count">{track.count.toLocaleString()} plays</span>
            <Movement track={track} rank={index + 1} compared={compared} />
          </div>
        </div>
      ))}
    </div>
  );
}

export function PopularRow({ track, rank, compared }) {
  return (
    <li className="chart-line">
      <span className="chart-line-rank">{rank}</span>
      <Cover url={track.cover_url} />
      <span className="chart-line-text">
        <span className="chart-line-title" title={track.title}>{track.title}</span>
        <span className="chart-line-artist">{track.artist}{track.album ? ` · ${track.album}` : ''}</span>
      </span>
      <Movement track={track} rank={rank} compared={compared} />
      <span className="chart-line-count">{track.count.toLocaleString()}</span>
    </li>
  );
}
