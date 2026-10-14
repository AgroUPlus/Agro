import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
// App before the expressive layer, so that layer's styles load last and win over every
// component stylesheet App pulls in (library.css, setup.css).
import App from './App.jsx'
import './expressive/type.css'
import './expressive/ambient.css'
import './expressive/shell.css'
import './expressive/surfaces.css'
import './expressive/controls.css'
import './expressive/nowbar.css'
import './expressive/stats.css'
import './expressive/charts.css'
import './expressive/backups.css'
import './expressive/devices.css'
import './expressive/social.css'
import './expressive/settings.css'
import './expressive/admin.css'
import './expressive/library.css'
import './expressive/auth.css'
import './expressive/phone.css'
import { startExpressiveCursor } from './expressive/cursor.js'

startExpressiveCursor()

createRoot(document.getElementById('root')).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
