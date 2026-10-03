import { ListMusic } from 'lucide-react';
import { itemArt } from './art.js';

/**
 * A playlist's cover: a 2×2 mosaic of the first four distinct album covers once it has four, the
 * first one alone before that, and an icon when none of its tracks has any art.
 */
export default function PlaylistCover({ items, size = 28 }) {
  const art = [...new Set(items.map(itemArt).filter(Boolean))];
  if (art.length >= 4) {
    return (
      <div className="mosaic">
        {art.slice(0, 4).map((src) => (
          <img key={src} src={src} alt="" loading="lazy" />
        ))}
      </div>
    );
  }
  if (art.length > 0) return <img src={art[0]} alt="" loading="lazy" />;
  return <ListMusic size={size} />;
}
