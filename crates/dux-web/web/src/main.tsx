import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { AuthGate } from './components/AuthGate.tsx'
import { RootErrorBoundary } from './components/RootErrorBoundary.tsx'
import { RowReasonContext } from './components/modal/rowReason.ts'
import { useRowChangeReason } from './hooks/use-agent-active-elsewhere.ts'
import { watchComposition } from './lib/clipboard.ts'
import { answerOpenedTabs } from './lib/openerPage.ts'
import { installShellLayoutAttribute } from './lib/shellLayout.ts'
import { registerServiceWorker } from './lib/sw.ts'

// dux's web UI is a dark, desktop-style app; opt into the `.dark` token set.
document.documentElement.classList.add('dark')
document.documentElement.style.colorScheme = 'dark'

// The `md:`/`max-md:` classes style for the layout this marks (lib/shellLayout.ts).
installShellLayoutAttribute(document)

// A copy must never take focus from a half-typed IME composition.
watchComposition(document)

// An editor tab opened from this page writes as this page (lib/openerPage.ts).
answerOpenedTabs()

// Offline-fallback PWA support (dormant on insecure origins; see lib/sw.ts).
registerServiceWorker()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {/* The last resort for a render error no inner boundary caught. */}
    <RootErrorBoundary>
      {/* The sign-in gate decides whether the app or a sign-in page is on screen. */}
      {/* Every modal whose action changes a row asks this whether it may. */}
      <RowReasonContext.Provider value={useRowChangeReason}>
        <AuthGate>
          <App />
        </AuthGate>
      </RowReasonContext.Provider>
    </RootErrorBoundary>
  </StrictMode>,
)
