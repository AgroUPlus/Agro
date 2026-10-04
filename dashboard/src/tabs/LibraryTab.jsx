import { useEffect, useState } from 'react';
import { ArrowLeft } from 'lucide-react';
import '../library.css';
import LibraryOverview from './LibraryOverview.jsx';
import LibraryBrowser from './LibraryBrowser.jsx';
import PlaylistDetail from './PlaylistDetail.jsx';
import AlbumDetail from './AlbumDetail.jsx';

/**
 * The Library tab and its pages, addressed by hash so the back button and a shared URL both work:
 * `#/library`, `#/library/playlist/<id>`, `#/library/album/<id>`, `#/library/browse`.
 */
function readRoute() {
  const [, page, ...rest] = window.location.hash.replace(/^#\/?/, '').split('/');
  const id = rest.length ? decodeURIComponent(rest.join('/')) : null;
  if ((page === 'playlist' || page === 'album') && id) return { page, id };
  if (page === 'browse') return { page };
  return { page: 'overview' };
}

const go = (...parts) => {
  window.location.hash = `#/${['library', ...parts.map(encodeURIComponent)].join('/')}`;
};

export default function LibraryTab({ username, devices, onUnauthorized }) {
  const [route, setRoute] = useState(readRoute);

  useEffect(() => {
    const onHash = () => setRoute(readRoute());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  const back = () => go();

  if (route.page === 'playlist') {
    return <PlaylistDetail key={route.id} id={route.id} me={username} onBack={back} onUnauthorized={onUnauthorized} />;
  }
  if (route.page === 'album') {
    return <AlbumDetail key={route.id} id={route.id} username={username} onBack={back} onUnauthorized={onUnauthorized} />;
  }
  if (route.page === 'browse') {
    return (
      <div>
        <button type="button" className="pill-btn detail-back" onClick={back}><ArrowLeft size={14} /> Library</button>
        <LibraryBrowser username={username} devices={devices} onUnauthorized={onUnauthorized} />
      </div>
    );
  }
  return (
    <LibraryOverview
      username={username}
      onOpenPlaylist={(id) => go('playlist', id)}
      onOpenAlbum={(id) => go('album', id)}
      onBrowseAll={() => go('browse')}
      onUnauthorized={onUnauthorized}
    />
  );
}
