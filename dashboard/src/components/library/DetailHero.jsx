/**
 * The top of a playlist or album page: big cover, what kind of thing this is, a title set large
 * enough to read across the room, and a byline.
 */
export default function DetailHero({ cover, kind, title, description, byline }) {
  return (
    <section className="detail-hero">
      <div className="detail-cover">{cover}</div>
      <div className="detail-meta">
        <div className="detail-kind">{kind}</div>
        <h2 className="detail-title">{title}</h2>
        {description && <div className="detail-description">{description}</div>}
        <div className="detail-byline">{byline}</div>
      </div>
    </section>
  );
}
