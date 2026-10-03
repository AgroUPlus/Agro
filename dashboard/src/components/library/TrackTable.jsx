import { useState } from 'react';
import { Music } from 'lucide-react';
import { formatClock, formatDate } from './format.js';

/**
 * The track list of a playlist or album page.
 *
 * `columns` says which optional columns to show: album, date added, and who added it (only worth
 * a column once others can edit). `onMove(from, to)` makes rows draggable — given only to those
 * allowed to reorder — and `actions(row, index)` renders a row's own buttons.
 */
export default function TrackTable({ rows, columns = {}, onMove, actions }) {
  const [dragFrom, setDragFrom] = useState(null);
  const [dropAt, setDropAt] = useState(null);

  const endDrag = () => {
    setDragFrom(null);
    setDropAt(null);
  };

  const dragProps = (index) =>
    onMove
      ? {
          draggable: true,
          onDragStart: (event) => {
            event.dataTransfer.effectAllowed = 'move';
            setDragFrom(index);
          },
          onDragOver: (event) => {
            if (dragFrom === null) return;
            event.preventDefault();
            setDropAt(index);
          },
          onDrop: (event) => {
            event.preventDefault();
            if (dragFrom !== null && dragFrom !== index) onMove(dragFrom, index);
            endDrag();
          },
          onDragEnd: endDrag
        }
      : {};

  return (
    <table className="track-table">
      <thead>
        <tr>
          <th className="track-num">#</th>
          <th>Title</th>
          {columns.album && <th className="col-album">Album</th>}
          {columns.added && <th className="col-added">Date added</th>}
          {columns.by && <th className="col-by">Added by</th>}
          <th className="track-duration">Time</th>
          {actions && <th className="track-row-actions" aria-label="Actions" />}
        </tr>
      </thead>
      <tbody>
        {rows.map((row, index) => (
          <tr key={row.key} className={dropAt === index && dragFrom !== index ? 'drop-target' : ''} {...dragProps(index)}>
            <td className="track-num">{row.number ?? index + 1}</td>
            <td>
              <div className="track-main">
                {columns.art !== false && (
                  <div className="track-thumb">
                    {row.art ? <img src={row.art} alt="" loading="lazy" /> : <Music size={16} />}
                  </div>
                )}
                <div style={{ minWidth: 0 }}>
                  <div className="track-title">{row.title}</div>
                  <div className="track-artist">{row.artist}</div>
                </div>
              </div>
            </td>
            {columns.album && <td className="col-album">{row.album}</td>}
            {columns.added && <td className="col-added">{formatDate(row.addedAt)}</td>}
            {columns.by && <td className="col-by">{row.addedBy}</td>}
            <td className="track-duration">{formatClock(row.durationMs)}</td>
            {actions && <td className="track-row-actions">{actions(row, index)}</td>}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
