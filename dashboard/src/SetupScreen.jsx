import { useEffect, useState } from 'react';
import { Copy, Check, Loader2, TerminalSquare, ShieldCheck, KeyRound } from 'lucide-react';
import { login } from './api.js';
import { bootstrapAdmin } from './setup.js';
import { unlockPresence } from './vault.js';
import Field from './components/form/Field.jsx';
import TextInput from './components/form/TextInput.jsx';
import { AuthShell } from './AuthScreen.jsx';
import './setup.css';

const STEPS = ['Create admin', 'Save passphrase', 'Sign in'];
const USERNAME_RULE = /^[a-z0-9._-]{1,32}$/;

/**
 * First-run setup for a server with no accounts.
 *
 * Three steps, each doing one thing: prove you own the server (its setup token) and name the
 * administrator; keep the passphrase the server will never show again; then sign in. Two-factor is
 * recommended, not required, so the dashboard suggests it once you are in.
 *
 * The token lives only in this component's state. It is never written to storage or the URL, and
 * the server burns it on the first success.
 */
export default function SetupScreen({ initialToken, onSignedIn }) {
  const [step, setStep] = useState(0);
  const [token, setToken] = useState(initialToken);
  const [username, setUsername] = useState('admin');
  const [created, setCreated] = useState(null);
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  // Once the passphrase is on screen, leaving the page loses it for good.
  useEffect(() => {
    if (step !== 1) return undefined;
    const warn = (event) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [step]);

  const cleanName = username.trim().toLowerCase();
  const nameProblem = cleanName && !USERNAME_RULE.test(cleanName)
    ? 'Use up to 32 letters, numbers, dots, dashes or underscores.'
    : '';

  async function createAdmin(event) {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      setCreated(await bootstrapAdmin(token, cleanName));
      setToken('');
      setStep(1);
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  }

  async function finish() {
    setBusy(true);
    setError('');
    try {
      const session = await login(created.username, created.passphrase);
      if (session.vaultSalt && session.vaultKeyWrapped) {
        await unlockPresence(created.passphrase, session.vaultSalt, session.vaultKeyWrapped)
          .catch(() => console.error('The vault key did not open with this passphrase'));
      }
      setCreated(null);
      onSignedIn();
    } catch (err) {
      setError(err.message);
      setBusy(false);
    }
  }

  function copy() {
    navigator.clipboard?.writeText(created.passphrase);
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  }

  return (
    <AuthShell subtitle="Set up your server">
      <ol className="setup-steps" aria-label="Setup progress">
        {STEPS.map((label, index) => (
          <li key={label} className={index < step ? 'done' : index === step ? 'current' : ''}
              aria-current={index === step ? 'step' : undefined}>
            <span className="setup-dot">{index < step ? <Check size={12} /> : index + 1}</span>
            <span className="setup-step-label">{label}</span>
          </li>
        ))}
      </ol>

      {step === 0 && (
        <form onSubmit={createAdmin} className="setup-body">
          <p className="auth-lede">
            Welcome. This server has no accounts yet, so you are the first. Create the administrator
            account; it can approve everyone who joins later.
          </p>

          <Field
            id="setup-token"
            label={<><TerminalSquare size={13} /> Setup token</>}
            hint="Printed in the server's log when it started. In Docker: docker logs agro. It works once."
          >
            {(field) => (
              <TextInput
                {...field}
                type="password"
                autoFocus={!initialToken}
                autoComplete="off"
                spellCheck={false}
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="paste the token from the log"
              />
            )}
          </Field>
          {initialToken && token === initialToken && (
            <p className="setup-linked"><Check size={13} /> Filled in from the link you opened.</p>
          )}

          <Field id="setup-username" label="Admin username" error={nameProblem}>
            {(field) => (
              <TextInput
                {...field}
                type="text"
                autoComplete="username"
                autoFocus={Boolean(initialToken)}
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
            )}
          </Field>

          {error && <p className="auth-error" role="alert">{error}</p>}
          <button type="submit" className="auth-submit"
                  disabled={busy || !token.trim() || !cleanName || Boolean(nameProblem)}>
            {busy && <Loader2 size={14} className="auth-spin" />}
            Create administrator
          </button>
        </form>
      )}

      {step === 1 && created && (
        <div className="setup-body">
          <p className="auth-lede">
            This is <strong>{created.username}</strong>'s passphrase. It is shown <strong>once</strong>:
            the server keeps only a hash and cannot show it again. Store it in a password manager now.
          </p>
          <div className="auth-passphrase">
            <code>{created.passphrase}</code>
            <button type="button" className="auth-copy" onClick={copy} aria-label="Copy passphrase">
              {copied ? <Check size={15} /> : <Copy size={15} />}
            </button>
          </div>

          <label className="setup-confirm">
            <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
            <span>I have saved this passphrase somewhere safe.</span>
          </label>

          <div className="auth-notice setup-next">
            <ShieldCheck size={15} />
            <span>We recommend adding two-factor sign-in once you are in. The dashboard will walk you through it.</span>
          </div>

          {error && <p className="auth-error" role="alert">{error}</p>}
          <button type="button" className="auth-submit" disabled={!saved || busy} onClick={finish}>
            {busy ? <Loader2 size={14} className="auth-spin" /> : <KeyRound size={14} />}
            Continue
          </button>
        </div>
      )}
    </AuthShell>
  );
}
