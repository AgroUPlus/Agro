import { PATHS } from './shapes.js';

/** A Material 3 Expressive shape (see shapes.js) with content centred on it. */
export default function Cookie({ shape = 'cookie', size, className = '', children }) {
  return (
    <span className={`m3-shape ${className}`} style={size ? { width: size, height: size } : undefined}>
      <svg className="m3-shape-bg" viewBox="0 0 100 100" aria-hidden="true">
        <path d={PATHS[shape]} />
      </svg>
      {children}
    </span>
  );
}
