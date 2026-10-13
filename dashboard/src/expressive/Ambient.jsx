/**
 * The scenery behind every screen: drifting aurora, a dot grid and, while something plays, its cover
 * washed across the top. Purely decorative, so it is hidden from assistive technology.
 */
export default function Ambient({ artworkUrl = '' }) {
  const art = artworkUrl ? { '--ambient-art': `url("${artworkUrl.replace(/"/g, '%22')}")` } : undefined;
  return (
    <div className="ambient" aria-hidden="true">
      <div className="ambient-blob violet" />
      <div className="ambient-blob mint" />
      <div className="ambient-blob lemon" />
      <div className={`ambient-art ${artworkUrl ? 'on' : ''}`} style={art} />
      <div className="ambient-grid" />
    </div>
  );
}
