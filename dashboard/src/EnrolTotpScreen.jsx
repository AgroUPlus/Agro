import { useEffect, useState } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import { Shield, Check, KeyRound, Loader2 } from 'lucide-react';
import { gql, setToken } from './api.js';
import Field from './components/form/Field.jsx';
import TextInput from './components/form/TextInput.jsx';

const BEGIN = `mutation { beginTotp { otpauthUri secretBase32 } }`;
const CONFIRM = `mutation Confirm($code: String!) {
  confirmTotp(code: $code) { recoveryCodes devicesSignedOut }
}`;

/**
 * The screen an administrator sees when this server requires a second factor and they have not
 * enrolled one yet.
 *
 * It exists because the server's gate refuses a whole GraphQL document, and every dashboard query
 * asks for several things at once — so before this, the refusal rendered as a dashboard with an
 * empty name, an empty profile and empty settings. Nothing said what was wrong or what to do, and
 * the one action that would fix it was buried in the settings page the gate had just emptied.
 *
 * Deliberately not dismissible. The account genuinely cannot do anything else, so offering a way
 * past would only lead back to the same empty screens.
 */
export default function EnrolTotpScreen({ onEnrolled }) {
  const [enrolment, setEnrolment] = useState(null);
  const [code, setCode] = useState('');
  const [touched, setTouched] = useState(false);
  const [recoveryCodes, setRecoveryCodes] = useState(null);
  const [signedOut, setSignedOut] = useState(0);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const res = await gql(BEGIN);
        const body = await res.json();
        if (body?.errors?.length) throw new Error(body.errors[0].message);
        setEnrolment(body?.data?.beginTotp);
      } catch (err) {
        setError(err.message);
      }
    })();
  }, []);

  async function confirm(event) {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      const res = await gql(CONFIRM, { code: code.trim() });
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      setRecoveryCodes(body.data.confirmTotp.recoveryCodes);
      setSignedOut(body.data.confirmTotp.devicesSignedOut);
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  }

  // Confirming revokes every other token, so the codes are shown before anything reloads —
  // there is no second chance to read them.
  if (recoveryCodes) {
    return (
      <Shell title="Save your recovery codes">
        <p className="auth-lede">
          These are shown <strong>once</strong>. Each works a single time and can be used in place
          of a code if you lose your authenticator. Without them, a lost phone is a lost account.
        </p>
        <div className="enrol-codes">
          {recoveryCodes.map((c) => <code key={c}>{c}</code>)}
        </div>
        {signedOut > 0 && (
          <div className="auth-notice">
            <strong>{signedOut} other {signedOut === 1 ? 'device was' : 'devices were'} signed
            out.</strong> Enrolling revokes credentials issued before the second factor existed —
            sign Wanda and Wander in again to reconnect them.
          </div>
        )}
        <button
          type="button"
          className="auth-submit"
          onClick={() => {
            navigator.clipboard?.writeText(recoveryCodes.join('\n'));
            onEnrolled();
          }}
        >
          <Check size={14} /> Copy and continue
        </button>
      </Shell>
    );
  }

  return (
    <Shell title="Set up two-factor authentication">
      <p className="auth-lede">
        This server requires administrators to use a second factor. Scan this with an authenticator
        app, then enter the code it shows.
      </p>

      {enrolment ? (
        <>
          <div className="enrol-qr">
            <QRCodeSVG value={enrolment.otpauthUri} size={168} level="M" />
          </div>
          <p className="auth-hint">Or enter this secret by hand:</p>
          <code className="enrol-secret">{enrolment.secretBase32}</code>

          <form onSubmit={confirm}>
            <Field
              id="enrol-code"
              label="Code from your app"
              error={
                touched && code.trim() && code.trim().length !== 6
                  ? 'The code is six digits.'
                  : ''
              }
            >
              {(field) => (
                <TextInput
                  {...field}
                  type="text"
                  autoFocus
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  onBlur={() => setTouched(true)}
                  placeholder="123456"
                />
              )}
            </Field>
            {error && <p className="auth-error" role="alert">{error}</p>}
            <button type="submit" className="auth-submit" disabled={busy || code.trim().length !== 6}>
              {busy ? <Loader2 size={14} className="auth-spin" /> : <Shield size={14} />}
              Verify and enable
            </button>
          </form>
        </>
      ) : error ? (
        <>
          <p className="auth-error" role="alert">{error}</p>
          <div className="auth-notice">
            <strong>If this says the server cannot store secrets,</strong> it has no{' '}
            <code>AGRO_SECRET_KEY</code> set. Two-factor secrets are encrypted at rest, and the
            server refuses to store them in the clear rather than pretending to protect them.
            Set one in the service environment and restart.
          </div>
        </>
      ) : (
        <p className="auth-lede"><Loader2 size={14} className="auth-spin" /> Preparing…</p>
      )}

      <button
        type="button"
        className="auth-secondary"
        onClick={() => { setToken(''); window.location.reload(); }}
      >
        Sign out instead
      </button>
    </Shell>
  );
}

function Shell({ title, children }) {
  return (
    <div className="auth-page">
      <div className="auth-card">
        <div className="auth-brand"><KeyRound size={18} /> Agro</div>
        <p className="auth-subtitle">{title}</p>
        {children}
      </div>
    </div>
  );
}
