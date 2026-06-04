import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { registerServiceWorker } from './lib/sw.ts'

// dux's web UI is a dark, desktop-style app; opt into the `.dark` token set.
document.documentElement.classList.add('dark')
document.documentElement.style.colorScheme = 'dark'

// Offline-fallback PWA support (dormant on insecure origins; see lib/sw.ts).
registerServiceWorker()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
