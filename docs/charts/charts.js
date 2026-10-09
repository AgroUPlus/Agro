const DEFAULT_SERVER = 'https://agro.kolbxyz.xyz';
const STORAGE_KEY = 'agro_charts_server';
const TOP_N = 50;
const SKELETON_ROWS = 12;
// The server only knows whole UTC days, so "last day" is today plus yesterday: a single bucket
// is empty just after midnight and rarely clears the exposure floor on a small fleet.
const WINDOWS = [
  { label: 'Last day', days: 2 },
  { label: '7 days', days: 7 },
  { label: '30 days', days: 30 },
];

let days = 7;
let requestId = 0;

function readStoredServer() {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch (e) {
    return null;
  }
}
function writeStoredServer(value) {
  try {
    localStorage.setItem(STORAGE_KEY, value);
  } catch (e) {
    // Private browsing, blocked storage, etc. — the ?server= override or the default still
    // work for this page load, so there is nothing to recover from here.
  }
}

function resolveServer() {
  const fromQuery = new URLSearchParams(window.location.search).get('server');
  if (fromQuery) return fromQuery.replace(/\/$/, '');
  const stored = readStoredServer();
  if (stored) return stored.replace(/\/$/, '');
  return DEFAULT_SERVER;
}

let server = resolveServer();

const serverDisplay = document.getElementById('server-display');
const chartCard = document.getElementById('chart-card');
const windowSwitcher = document.getElementById('window-switcher');
const serverBackdrop = document.getElementById('server-backdrop');
const serverInput = document.getElementById('server-input');

const dashboardLink = document.getElementById('dashboard-link');

function renderServerDisplay() {
  serverDisplay.textContent = server;
  dashboardLink.href = server;
}

const windowButtons = [];
const thumb = el('span', 'seg-thumb');

// Built once: the highlight is a single element that glides between buttons, which is what makes
// the selection read as one moving thing rather than one pill vanishing and another appearing.
function buildWindowSwitcher() {
  windowSwitcher.append(thumb);
  for (const option of WINDOWS) {
    const btn = el('button', 'segmented-btn', option.label);
    btn.onclick = () => {
      if (days === option.days) return;
      days = option.days;
      syncWindowSwitcher();
      loadChart();
    };
    windowButtons.push(btn);
    windowSwitcher.append(btn);
  }
  syncWindowSwitcher();
  window.addEventListener('resize', syncWindowSwitcher);
  // Button widths change once the web font arrives.
  if (document.fonts) document.fonts.ready.then(syncWindowSwitcher);
}

function syncWindowSwitcher() {
  WINDOWS.forEach((option, i) => {
    const active = option.days === days;
    windowButtons[i].classList.toggle('active', active);
    windowButtons[i].setAttribute('aria-pressed', String(active));
    if (active) {
      thumb.style.width = `${windowButtons[i].offsetWidth}px`;
      thumb.style.height = `${windowButtons[i].offsetHeight}px`;
      thumb.style.transform = `translateX(${windowButtons[i].offsetLeft}px)`;
    }
  });
}

// A search link, not a specific video: the server never claims to know which upload is
// "the" recording, since that identifier would have to come from someone's own library and
// this chart is deliberately anonymous. See docs on `/api/v1/popular` for why.
function youtubeMusicSearchUrl(artist, title) {
  return `https://music.youtube.com/search?q=${encodeURIComponent(`${artist} ${title}`)}`;
}

function coverPlaceholderIcon() {
  const icon = document.createElement('span');
  icon.className = 'material-symbols-rounded';
  icon.textContent = 'music_note';
  return icon;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function setHint(text) {
  chartCard.setAttribute('aria-busy', 'false');
  chartCard.replaceChildren(el('div', 'empty-hint', text));
}

function showSkeleton() {
  chartCard.setAttribute('aria-busy', 'true');
  const list = el('div', 'chart-list chart-skeleton-list');
  for (let i = 0; i < SKELETON_ROWS; i++) {
    const row = el('div', 'chart-row chart-skeleton');
    row.style.setProperty('--i', i);
    row.append(el('div', 'bone bone-cover'));
    const info = el('div', 'chart-info');
    info.style.gap = '8px';
    const a = el('div', 'bone bone-line');
    a.style.width = `${45 + ((i * 17) % 30)}%`;
    const b = el('div', 'bone bone-line');
    b.style.width = `${25 + ((i * 13) % 20)}%`;
    info.append(a, b);
    row.append(info);
    list.append(row);
  }
  chartCard.replaceChildren(list);
}

// Rank movement versus the window before this one. Shown only when the server says the comparison
// is meaningful; a missing previous rank then means the track is a new entry.
function movementBadge(track, index) {
  const badge = el('span', 'chart-move');
  if (track.previous_rank == null) {
    badge.classList.add('new');
    badge.textContent = 'NEW';
    badge.title = 'Not on the chart in the previous period';
    return badge;
  }
  const delta = track.previous_rank - (index + 1);
  if (delta === 0) {
    badge.classList.add('same');
    badge.textContent = '–';
    badge.title = 'Same rank as the previous period';
    return badge;
  }
  badge.classList.add(delta > 0 ? 'up' : 'down');
  badge.textContent = `${delta > 0 ? '▲' : '▼'} ${Math.abs(delta)}`;
  badge.title = `${delta > 0 ? 'Up' : 'Down'} from #${track.previous_rank} in the previous period`;
  return badge;
}

function renderTrack(track, index, compared) {
  const row = el('a', 'chart-row enter' + (index < 3 ? ' top' : ''));
  row.style.setProperty('--i', index);
  row.href = youtubeMusicSearchUrl(track.artist, track.title);
  row.target = '_blank';
  row.rel = 'noopener';
  row.title = `Search "${track.artist} - ${track.title}" on YouTube Music`;

  row.append(el('span', 'chart-rank', String(index + 1)));

  const cover = el('div', 'chart-cover');
  if (track.cover_url) {
    const img = document.createElement('img');
    img.src = track.cover_url;
    img.alt = '';
    img.decoding = 'async';
    // Fades in once decoded, so a cover never pops in half-way through its row's entrance.
    img.className = 'cover-img';
    img.onload = () => img.classList.add('loaded');
    // A dead image link falls back to the same placeholder icon a missing cover gets.
    img.onerror = () => cover.replaceChildren(coverPlaceholderIcon());
    cover.append(img);
  } else {
    cover.append(coverPlaceholderIcon());
  }
  row.append(cover);

  const info = el('div', 'chart-info');
  const artistLine = track.album ? `${track.artist} · ${track.album}` : track.artist;
  const title = el('span', 'chart-title', track.title);
  title.title = track.title;
  const artist = el('span', 'chart-artist', artistLine);
  artist.title = artistLine;
  info.append(title, artist);
  row.append(info);
  if (compared) row.append(movementBadge(track, index));
  return row;
}

function renderTracks(tracks, compared) {
  chartCard.setAttribute('aria-busy', 'false');
  const list = el('div', 'chart-list');
  tracks.forEach((track, index) => list.append(renderTrack(track, index, compared)));
  chartCard.replaceChildren(list);
}

function loadChart() {
  const mine = ++requestId;
  // Switching windows dims the current ranking instead of blanking it, so covers already on
  // screen are not torn down and re-fetched while the next list is on its way.
  const current = chartCard.querySelector('.chart-list:not(.chart-skeleton-list)');
  if (current && current.querySelector('.chart-row')) {
    current.classList.add('refreshing');
  } else {
    showSkeleton();
  }
  const stale = () => mine !== requestId;
  fetch(`${server}/api/v1/popular?days=${days}&limit=${TOP_N}`)
    .then((response) => (response.ok ? response.json() : null))
    .then((body) => {
      if (stale()) return;
      if (!body) return setHint("Couldn't reach that server. Check the address and try again.");
      if (!body.enabled) return setHint('Charts are turned off on this server.');
      if (!body.tracks || body.tracks.length === 0) return setHint('Nothing charted yet.');
      renderTracks(body.tracks, body.compared === true);
    })
    .catch(() => {
      if (!stale()) setHint("Couldn't reach that server. Check the address and try again.");
    });
}

function openServerModal() {
  serverInput.value = server;
  serverBackdrop.classList.add('open');
  serverInput.focus();
}
function closeServerModal() {
  serverBackdrop.classList.remove('open');
}
function saveServer() {
  let val = serverInput.value.trim();
  if (!val) return;
  if (!/^https?:\/\//i.test(val)) val = 'https://' + val;
  server = val.replace(/\/$/, '');
  writeStoredServer(server);
  renderServerDisplay();
  closeServerModal();
  loadChart();
}

serverBackdrop.addEventListener('click', (e) => {
  if (e.target === serverBackdrop) closeServerModal();
});
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') closeServerModal();
});

renderServerDisplay();
buildWindowSwitcher();
loadChart();
