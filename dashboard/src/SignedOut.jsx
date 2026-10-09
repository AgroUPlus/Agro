import { useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';
import AuthScreen from './AuthScreen.jsx';
import SetupScreen from './SetupScreen.jsx';
import { setupStatus, readSetupFragment, clearSetupFragment } from './setup.js';

/**
 * What a signed-out visitor sees: the first-run setup on a brand-new server, the sign-in page on
 * every other one. Decided by the server, so a configured instance never shows a setup screen.
 */
export default function SignedOut(props) {
  // Read once, then scrubbed from the address bar in the effect below.
  const [linkedToken] = useState(readSetupFragment);
  const [needsSetup, setNeedsSetup] = useState(null);

  useEffect(() => {
    clearSetupFragment();
    setupStatus().then(setNeedsSetup);
  }, []);

  if (needsSetup === null) {
    return (
      <div className="auth-page" aria-busy="true">
        <Loader2 size={22} className="auth-spin" />
      </div>
    );
  }
  if (needsSetup) {
    return <SetupScreen initialToken={linkedToken} onSignedIn={props.onSignedIn} />;
  }
  return <AuthScreen {...props} />;
}
