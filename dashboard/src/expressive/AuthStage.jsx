import { Smartphone, Laptop, Tablet, Headphones, Speaker, Globe } from 'lucide-react';
import Cookie from './Cookie.jsx';
import { cookiePath } from './shapes.js';

/** Each device rides a ring on its own M3 Expressive shape, in its own tonal colour. */
const ORBITERS = [
  { icon: Smartphone, ring: 'inner', at: 0, shape: 'cookie', tone: 'primary' },
  { icon: Speaker, ring: 'inner', at: 180, shape: 'circle', tone: 'pink' },
  { icon: Laptop, ring: 'middle', at: 120, shape: 'clover', tone: 'tertiary' },
  { icon: Tablet, ring: 'middle', at: 300, shape: 'flower', tone: 'lemon' },
  { icon: Headphones, ring: 'outer', at: 210, shape: 'sunny', tone: 'secondary' },
];

/** The rings themselves wave, like M3 Expressive's circular progress indicator. */
const RING_PATHS = {
  inner: cookiePath(30, 0.012, 720),
  middle: cookiePath(38, 0.01, 720),
  outer: cookiePath(46, 0.008, 720),
};

/** GitHub's mark. lucide no longer ships brand logos, so it is drawn here. */
function GitHubMark() {
  return (
    <svg viewBox="0 0 16 16" width="15" height="15" fill="currentColor" aria-hidden="true">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

/**
 * The signed-out page around the sign-in or setup card: what this server is, set like the promo
 * site's hero, and the card itself held in an orbit of devices, the thing Agro keeps in sync.
 */
export default function AuthStage({ children }) {
  return (
    <div className="auth-stage">
      <section className="auth-hero">
        <h1 className="auth-hero-title">
          Your music,
          <br />
          <span>on every device you own.</span>
        </h1>
        <p className="auth-hero-sub">
          Start on your desktop, pick up on your phone, listen along with friends, on a server
          you run, with every social switch off until you turn it on.
        </p>
      </section>

      <section className="auth-orbit-stage">
        <div className="auth-orbit" aria-hidden="true">
          {['inner', 'middle', 'outer'].map((ring) => (
            <div key={ring} className={`orbit-ring ${ring}`}>
              <svg className="orbit-wave" viewBox="0 0 100 100">
                <path d={RING_PATHS[ring]} />
              </svg>
              {ORBITERS.filter((o) => o.ring === ring).map(({ icon: Icon, at, shape, tone }) => (
                <span key={at} className={`orbit-node orbit-${tone}`} style={{ '--at': `${at}deg` }}>
                  <Cookie shape={shape} size={56}>
                    <Icon size={22} strokeWidth={2.2} />
                  </Cookie>
                </span>
              ))}
            </div>
          ))}
        </div>
        {children}
      </section>

      <footer className="auth-stage-footer">
        <a className="auth-footer-link" href="https://github.com/AgroUPlus/Agro" target="_blank" rel="noreferrer">
          <GitHubMark />
          GitHub
        </a>
        <a className="auth-footer-link" href="https://agrouplus.github.io/Agro" target="_blank" rel="noreferrer">
          <Globe size={15} />
          Website
        </a>
      </footer>
    </div>
  );
}
