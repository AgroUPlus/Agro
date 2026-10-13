import { useEffect, useState } from 'react';

function readPage(tab, pages, moved) {
  const [first, second] = window.location.hash.replace(/^#\/?/, '').split('/');
  if (moved[first]) return moved[first];
  return first === tab && pages.includes(second) ? second : pages[0];
}

/**
 * A tab's sub-page, kept in the address as `#/<tab>/<page>` so a link or the back button opens it.
 * The first page is the tab's own address. `moved` names old top-level tabs that now live here as a
 * page — `{ backups: 'backups' }` keeps an old `#/backups` link working.
 */
export function useSubPage(tab, pages, moved = {}) {
  const [page, setPage] = useState(() => readPage(tab, pages, moved));

  // Read once per hash change; `pages` and `moved` are constants at every call site.
  useEffect(() => {
    const follow = () => setPage(readPage(tab, pages, moved));
    window.addEventListener('hashchange', follow);
    return () => window.removeEventListener('hashchange', follow);
  }, [tab]); // eslint-disable-line react-hooks/exhaustive-deps

  const open = (next) => {
    window.location.hash = next === pages[0] ? `#/${tab}` : `#/${tab}/${next}`;
  };
  return [page, open];
}
