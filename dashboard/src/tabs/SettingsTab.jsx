import { Link2, UserRound } from 'lucide-react';
import SubTabs from '../expressive/SubTabs.jsx';
import { useSubPage } from '../expressive/useSubPage.js';
import AccountSettingsTab from './AccountSettingsTab.jsx';
import LinksTab from './LinksTab.jsx';

const PAGES = [
  { id: 'account', label: 'Account', icon: UserRound },
  { id: 'links', label: 'Share links', icon: Link2 }
];

/**
 * The account's own settings, and the share links it has made. Links had a tab of their own, but
 * they are set up once and seldom revisited, so they live here and an old `#/links` link opens them.
 */
export default function SettingsTab({ username, onUnauthorized }) {
  const [page, openPage] = useSubPage('settings', PAGES.map(p => p.id), { links: 'links' });
  return (
    <>
      <SubTabs pages={PAGES} active={page} onOpen={openPage} />
      {page === 'account' && <AccountSettingsTab username={username} onUnauthorized={onUnauthorized} />}
      {page === 'links' && <LinksTab username={username} onUnauthorized={onUnauthorized} />}
    </>
  );
}
