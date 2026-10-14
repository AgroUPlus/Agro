import { Users, Layers, ScrollText } from 'lucide-react';
import SubTabs from '../expressive/SubTabs.jsx';
import { useSubPage } from '../expressive/useSubPage.js';
import PeopleTab from './PeopleTab.jsx';
import AdminPluginsTab from './AdminPluginsTab.jsx';
import LogsTab from './LogsTab.jsx';

const PAGES = [
  { id: 'people', label: 'People', icon: Users },
  { id: 'features', label: 'Features', icon: Layers },
  { id: 'logs', label: 'Logs', icon: ScrollText }
];

export default function ManagementTab({ me, logs = [], onUnauthorized }) {
  const [page, openPage] = useSubPage('management', PAGES.map(p => p.id));
  return (
    <>
      <SubTabs pages={PAGES} active={page} onOpen={openPage} />
      {page === 'people' && <PeopleTab me={me} onUnauthorized={onUnauthorized} />}
      {page === 'features' && <AdminPluginsTab onUnauthorized={onUnauthorized} />}
      {page === 'logs' && <LogsTab logs={logs} />}
    </>
  );
}
