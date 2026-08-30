import { useEffect, useState } from 'react';
import { Copy, Check, ArrowLeft, Loader2, KeyRound, ShieldCheck } from 'lucide-react';
import { login, ssoConfig, TotpRequiredError } from './api.js';

/**
 * The whole of the signed-out experience: signing in, and creating an account.
 *
 * It is a separate component rather than a branch inside `App` because it is the only screen a
 * stranger ever sees. A public instance is judged here — this is the page someone lands on before
 * they have any reason to trust the server, and "type your four-word passphrase into this bare
 * input" was not an answer.
 *
 * The signup half exists because `/api/v1/signup` had no client at all. The account it creates is
 * usually `pending`, which is the part people get wrong: nothing is broken, an administrator has
 * simply not let them in yet, and the screen has to say so in those words.
 */
export default function AuthScreen({ onSignedIn, ssoError, onDismissSsoError }) {
  const [mode, setMode] = useState('signin');
  const [username, setUsername] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [inviteCode, setInviteCode] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  /** Set once a signup succeeds. Holds the passphrase the server will never show again. */
  const [created, setCreated] = useState(null);
  const [copied, setCopied] = useState(false);
  /**
   * Set only after the passphrase has already been accepted. Showing the code field any earlier
   * would tell a stranger which usernames exist and which have a second factor.
   */
  const [needsCode, setNeedsCode] = useState(false);
  const [totpCode, setTotpCode] = useState('');
  const [sso, setSso] = useState({ enabled: false });

  useEffect(() => {
    ssoConfig().then(setSso);
  }, []);

  async function handleSignIn(event) {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      await login(username, passphrase, needsCode ? totpCode : undefined);
      onSignedIn();
    } catch (err) {
      if (err instanceof TotpRequiredError) {
        // The passphrase was right. Ask for the code and keep everything else as it was, so the
        // second attempt can resend the passphrase without the user retyping it.
        setNeedsCode(true);
        setTotpCode('');
        setError(needsCode ? 'That code was not accepted. Try the next one.' : '');
      } else {
        setNeedsCode(false);
        setError(err.message);
      }
    } finally {
      setBusy(false);
    }
  }

  async function handleSignUp(event) {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      const res = await fetch('/api/v1/signup', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          username: username.trim(),
          ...(inviteCode.trim() ? { invite_code: inviteCode.trim() } : {})
        })
      });
      const body = await res.json().catch(() => ({}));
      if (!res.ok || !body.passphrase) {
        throw new Error(body.error || 'That account could not be created');
      }
      setCreated(body);
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  }

  function copyPassphrase() {
    navigator.clipboard?.writeText(created.passphrase);
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  }

  // ── The one-time passphrase, after a successful signup ────────────────────────────────────
  if (created) {
    const pending = String(created.state).toLowerCase() === 'pending';
    return (
      <AuthShell subtitle={`Welcome, ${created.username}`}>
        <p className="auth-lede">
          This is your passphrase. It is shown <strong>once</strong> — the server keeps only a hash
          of it and genuinely cannot show it again. Save it somewhere before you leave this page.
        </p>

        <div className="auth-passphrase">
          <code>{created.passphrase}</code>
          <button type="button" className="auth-copy" onClick={copyPassphrase} aria-label="Copy passphrase">
            {copied ? <Check size={15} /> : <Copy size={15} />}
          </button>
        </div>

        {pending ? (
          <div className="auth-notice">
            <strong>Waiting for approval.</strong> This server reviews new accounts, so signing in
            will be refused until an administrator lets you in. Nothing is wrong — try again later.
          </div>
        ) : (
          <div className="auth-notice auth-notice-ok">
            <strong>Your account is active.</strong> Sign in with the passphrase above.
          </div>
        )}

        <button
          type="button"
          className="auth-submit"
          onClick={() => {
            setCreated(null);
            setMode('signin');
            setPassphrase('');
          }}
        >
          Continue to sign in
        </button>
      </AuthShell>
    );
  }

  const signingUp = mode === 'signup';

  return (
    <AuthShell subtitle={signingUp ? 'Create an account' : 'Sign in to continue'}>
      <form onSubmit={signingUp ? handleSignUp : handleSignIn}>
        <label className="auth-label" htmlFor="auth-username">Username</label>
        <input
          id="auth-username"
          className="auth-input"
          type="text"
          autoFocus
          autoComplete="username"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          placeholder="username"
        />

        {signingUp ? (
          <>
            <label className="auth-label" htmlFor="auth-invite">
              Invite code <span className="auth-optional">optional</span>
            </label>
            <input
              id="auth-invite"
              className="auth-input"
              type="text"
              value={inviteCode}
              onChange={(e) => setInviteCode(e.target.value)}
              placeholder="skips the approval queue"
            />
          </>
        ) : (
          <>
            <label className="auth-label" htmlFor="auth-passphrase">Passphrase</label>
            <input
              id="auth-passphrase"
              className="auth-input"
              type="password"
              autoComplete="current-password"
              value={passphrase}
              onChange={(e) => setPassphrase(e.target.value)}
              placeholder="four-word-pass-phrase"
            />

            {needsCode && (
              <>
                <label className="auth-label" htmlFor="auth-totp">
                  <ShieldCheck size={13} /> Authenticator code
                </label>
                <input
                  id="auth-totp"
                  className="auth-input"
                  type="text"
                  autoFocus
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  value={totpCode}
                  onChange={(e) => setTotpCode(e.target.value)}
                  placeholder="123456"
                />
                <p className="auth-hint">
                  Lost your authenticator? Use one of your recovery codes here instead.
                </p>
              </>
            )}
          </>
        )}

        {ssoError && (
          <p className="auth-error" role="alert" onClick={onDismissSsoError}>
            {ssoError}
          </p>
        )}
        {error && <p className="auth-error" role="alert">{error}</p>}

        <button type="submit" className="auth-submit" disabled={busy}>
          {busy && <Loader2 size={14} className="auth-spin" />}
          {signingUp ? 'Create account' : 'Log in'}
        </button>
      </form>

      <div className="auth-divider"><span>or</span></div>

      {sso.enabled && !signingUp && (
        <a className="auth-secondary auth-sso" href="/api/v1/oidc/start">
          <KeyRound size={14} /> Continue with {sso.displayName}
        </a>
      )}

      <button
        type="button"
        className="auth-secondary"
        onClick={() => {
          setMode(signingUp ? 'signin' : 'signup');
          setError('');
        }}
      >
        {signingUp ? (<><ArrowLeft size={14} /> Back to sign in</>) : 'Create an account'}
      </button>
    </AuthShell>
  );
}

function AuthShell({ subtitle, children }) {
  return (
    <div className="auth-page">
      <div className="auth-card">
        <div className="auth-brand">Agro</div>
        <p className="auth-subtitle">{subtitle}</p>
        {children}
      </div>
      <p className="auth-footnote">Your own instance. Your own music.</p>
    </div>
  );
}
