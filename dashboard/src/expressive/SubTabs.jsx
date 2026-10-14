/** A tab's sub-pages as one pill bar: `pages` is `[{ id, label, icon }]`. */
export default function SubTabs({ pages, active, onOpen, children }) {
  return (
    <div className="subtabs-row">
      <div className="segmented" role="tablist">
        {pages.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            role="tab"
            aria-selected={active === id}
            className={`segmented-btn ${active === id ? 'active' : ''}`}
            onClick={() => onOpen(id)}
          >
            {Icon && <Icon size={15} />}
            {label}
          </button>
        ))}
      </div>
      {children}
    </div>
  );
}
