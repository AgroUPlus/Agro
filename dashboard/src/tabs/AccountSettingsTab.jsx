import React, { useCallback, useEffect, useState } from 'react';
import { gql } from '../api.js';
import ProfileSection from './settings/ProfileSection.jsx';
import SecuritySection from './settings/SecuritySection.jsx';
import SyncedPreferencesSection from './settings/SyncedPreferencesSection.jsx';
import DataRetentionSection from './settings/DataRetentionSection.jsx';

const PROFILE_SETTINGS_QUERY = `query AccountSettings($username: String!) {
  profile(username: $username) {
    username displayName bio avatarUrl showNowPlaying showStats discoverable showActivity
  }
  hasTotp
  syncedSettings(userId: $username) {
    serverUrl serverUsername lrclibUrl lyricsFetchOnline streamFormat shareDomain shareHosts shareEnabled
  }
}`;

const UPDATE_PROFILE = `mutation UpdateProfile($displayName: String, $bio: String, $avatarUrl: String) {
  updateProfile(displayName: $displayName, bio: $bio, avatarUrl: $avatarUrl) { username }
}`;

const SET_VISIBILITY = `mutation SetVisibility($showNowPlaying: Boolean, $showStats: Boolean, $discoverable: Boolean, $showActivity: Boolean) {
  setVisibility(showNowPlaying: $showNowPlaying, showStats: $showStats, discoverable: $discoverable, showActivity: $showActivity) { username }
}`;

const UPDATE_SYNCED_SETTINGS = `mutation UpdateSynced($input: SyncedSettingsInput!) {
  updateSyncedSettings(input: $input) { updatedAt }
}`;

const PURGE_SCROBBLES = `mutation Purge($userId: String!, $year: Int) {
  purgeScrobbles(userId: $userId, year: $year) { purgedCount success }
}`;

const BEGIN_TOTP = `mutation BeginTotp {
  beginTotp { otpauthUri secretBase32 }
}`;

const CONFIRM_TOTP = `mutation ConfirmTotp($code: String!) {
  confirmTotp(code: $code) { recoveryCodes devicesSignedOut }
}`;

// Disabling needs proof. A stolen device token did not pass a second factor, so letting one remove
// the second factor would mean the feature protected nothing but itself.
const DISABLE_TOTP = `mutation DisableTotp($code: String!) {
  disableTotp(code: $code)
}`;

const REGENERATE_RECOVERY = `mutation Regenerate($code: String!) {
  regenerateRecoveryCodes(code: $code)
}`;

export default function AccountSettingsTab({ username, onUnauthorized }) {
  const [profile, setProfile] = useState({ displayName: '', bio: '', avatarUrl: '' });
  const [visibility, setVisibility] = useState({ showNowPlaying: true, showStats: true, discoverable: true, showActivity: true });
  const [synced, setSynced] = useState({ serverUrl: '', serverUsername: '', lrclibUrl: '', streamFormat: 'FLAC', shareDomain: '', shareHosts: '', shareEnabled: true });

  const [hasTotp, setHasTotp] = useState(false);
  const [totpEnrolment, setTotpEnrolment] = useState(null);
  const [totpCode, setTotpCode] = useState('');
  const [totpNotice, setTotpNotice] = useState('');
  /** Shown once, immediately after enrolling. The server keeps only digests of these. */
  const [recoveryCodes, setRecoveryCodes] = useState(null);
  const [disableCode, setDisableCode] = useState('');

  const [profileSaved, setProfileSaved] = useState(false);
  const [syncedSaved, setSyncedSaved] = useState(false);
  const [purgeNotice, setPurgeNotice] = useState('');
  const [purgeYear, setPurgeYear] = useState('');

  const load = useCallback(async () => {
    try {
      const res = await gql(PROFILE_SETTINGS_QUERY, { username });
      const data = (await res.json())?.data;
      if (data?.profile) {
        setProfile({
          displayName: data.profile.displayName || '',
          bio: data.profile.bio || '',
          avatarUrl: data.profile.avatarUrl || ''
        });
        setVisibility({
          showNowPlaying: !!data.profile.showNowPlaying,
          showStats: !!data.profile.showStats,
          discoverable: !!data.profile.discoverable,
          showActivity: !!data.profile.showActivity
        });
      }
      if (data?.hasTotp !== undefined) setHasTotp(data.hasTotp);
      if (data?.syncedSettings) setSynced({ ...data.syncedSettings });
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    }
  }, [username, onUnauthorized]);

  useEffect(() => { load(); }, [load]);

  const handleSaveProfile = async (e) => {
    e.preventDefault();
    try {
      await gql(UPDATE_PROFILE, {
        displayName: profile.displayName.trim() || null,
        bio: profile.bio.trim() || null,
        avatarUrl: profile.avatarUrl.trim() || null
      });
      await gql(SET_VISIBILITY, visibility);
      setProfileSaved(true);
      setTimeout(() => setProfileSaved(false), 2500);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else alert(err.message);
    }
  };

  const handleSaveSynced = async (e) => {
    e.preventDefault();
    try {
      await gql(UPDATE_SYNCED_SETTINGS, {
        input: {
          userId: username,
          serverUrl: synced.serverUrl,
          serverUsername: synced.serverUsername,
          lrclibUrl: synced.lrclibUrl,
          lyricsFetchOnline: true,
          streamFormat: synced.streamFormat,
          shareDomain: synced.shareDomain || '',
          shareHosts: synced.shareHosts || '',
          shareEnabled: synced.shareEnabled
        }
      });
      setSyncedSaved(true);
      setTimeout(() => setSyncedSaved(false), 2500);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else alert(err.message);
    }
  };

  const handleBeginTotp = async () => {
    try {
      setTotpNotice('');
      const res = await gql(BEGIN_TOTP);
      const body = await res.json();
      setTotpEnrolment(body?.data?.beginTotp);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else setTotpNotice(err.message);
    }
  };

  const handleConfirmTotp = async () => {
    if (!totpCode.trim()) return;
    try {
      setTotpNotice('');
      const res = await gql(CONFIRM_TOTP, { code: totpCode.trim() });
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      const result = body?.data?.confirmTotp;
      setHasTotp(true);
      setTotpEnrolment(null);
      setTotpCode('');
      setRecoveryCodes(result?.recoveryCodes ?? []);
      const signedOut = result?.devicesSignedOut ?? 0;
      setTotpNotice(
        signedOut > 0
          ? `Two-factor authentication is on. ${signedOut} other ${signedOut === 1 ? 'device was' : 'devices were'} signed out — sign them in again to reconnect.`
          : 'Two-factor authentication is on.'
      );
    } catch (err) {
      setTotpNotice(err.message);
    }
  };

  const handleDisableTotp = async () => {
    if (!disableCode.trim()) {
      setTotpNotice('Enter a current code (or a recovery code) to turn this off.');
      return;
    }
    if (!window.confirm('Turn off two-factor authentication for this account?')) return;
    try {
      const res = await gql(DISABLE_TOTP, { code: disableCode.trim() });
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      setHasTotp(false);
      setTotpEnrolment(null);
      setRecoveryCodes(null);
      setDisableCode('');
      setTotpNotice('Two-factor authentication is off.');
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else setTotpNotice(err.message);
    }
  };

  const handleRegenerateRecovery = async () => {
    if (!disableCode.trim()) {
      setTotpNotice('Enter a current code to issue a new set.');
      return;
    }
    try {
      const res = await gql(REGENERATE_RECOVERY, { code: disableCode.trim() });
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      setRecoveryCodes(body?.data?.regenerateRecoveryCodes ?? []);
      setDisableCode('');
      setTotpNotice('New recovery codes issued. The previous set no longer works.');
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else setTotpNotice(err.message);
    }
  };

  const handlePurgeScrobbles = async () => {
    const yr = purgeYear ? parseInt(purgeYear, 10) : null;
    const desc = yr ? `year ${yr}` : 'ALL-TIME history';
    if (!window.confirm(`⚠️ WARNING: Permanently delete listening history for ${desc}?\n\nThis cannot be undone.`)) return;
    try {
      const res = await gql(PURGE_SCROBBLES, { userId: username, year: yr });
      const body = await res.json();
      const count = body?.data?.purgeScrobbles?.purgedCount ?? 0;
      setPurgeNotice(`Successfully purged ${count} scrobbles for ${desc}.`);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else alert(err.message);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '20px', maxWidth: '840px' }}>
      <ProfileSection
        profile={profile}
        onProfileChange={setProfile}
        visibility={visibility}
        onVisibilityChange={setVisibility}
        onSave={handleSaveProfile}
        saved={profileSaved}
      />

      <SecuritySection
        hasTotp={hasTotp}
        totpEnrolment={totpEnrolment}
        totpCode={totpCode}
        onTotpCodeChange={setTotpCode}
        totpNotice={totpNotice}
        recoveryCodes={recoveryCodes}
        onDismissRecoveryCodes={() => setRecoveryCodes(null)}
        disableCode={disableCode}
        onDisableCodeChange={setDisableCode}
        onRegenerateRecovery={handleRegenerateRecovery}
        onBeginTotp={handleBeginTotp}
        onConfirmTotp={handleConfirmTotp}
        onDisableTotp={handleDisableTotp}
      />

      <SyncedPreferencesSection
        synced={synced}
        onSyncedChange={setSynced}
        onSave={handleSaveSynced}
        saved={syncedSaved}
      />

      <DataRetentionSection
        purgeYear={purgeYear}
        onPurgeYearChange={setPurgeYear}
        onPurgeScrobbles={handlePurgeScrobbles}
        purgeNotice={purgeNotice}
      />
    </div>
  );
}
