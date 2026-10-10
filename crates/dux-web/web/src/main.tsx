import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { AuthGate } from './components/AuthGate.tsx'
import { RootErrorBoundary } from './components/RootErrorBoundary.tsx'
import { registerServiceWorker } from './lib/sw.ts'

// dux's web UI is a dark, desktop-style app; opt into the `.dark` token set.
document.documentElement.classList.add('dark')
document.documentElement.style.colorScheme = 'dark'

// Offline-fallback PWA support (dormant on insecure origins; see lib/sw.ts).
registerServiceWorker()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {/* The last resort for a render error no inner boundary caught. */}
    <RootErrorBoundary>
      {/* The sign-in gate decides whether the app or a sign-in page is on screen. */}
      <AuthGate>
        <App />
      </AuthGate>
    </RootErrorBoundary>
  </StrictMode>,
)
