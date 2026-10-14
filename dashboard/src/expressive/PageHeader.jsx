import { Sparkles } from 'lucide-react';
import Cookie from './Cookie.jsx';

function greeting(hour) {
  if (hour < 5) return 'Up late';
  if (hour < 12) return 'Good morning';
  if (hour < 18) return 'Good afternoon';
  return 'Good evening';
}

/**
 * A page title in Wanda's voice: the tab's icon in an M3 Expressive cookie, a large rounded
 * headline and one line on what the page is for. Keyed on the tab, so each change of page replays
 * the entrance.
 */
export default function PageHeader({ tab, username }) {
  const Icon = tab?.icon ?? Sparkles;
  const hello = greeting(new Date().getHours());
  return (
    <header className="page-header" key={tab?.id}>
      <p className="page-greeting">{username ? `${hello}, ${username}` : hello}</p>
      <div className="page-title-row">
        <Cookie className="page-shape" size={60}>
          <Icon size={26} strokeWidth={2.4} />
        </Cookie>
        <h1 className="page-title">{tab?.label ?? 'Agro'}</h1>
      </div>
      {tab?.blurb && <p className="page-blurb">{tab.blurb}</p>}
    </header>
  );
}
