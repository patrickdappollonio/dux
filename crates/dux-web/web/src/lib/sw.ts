// Service-worker registration for dux's offline-fallback PWA support.
//
// This is pure progressive enhancement: if it works, navigations fall back to a
// branded offline page when the server is unreachable; if it doesn't, the app
// behaves exactly as before. So every failure is swallowed silently.
//
// We require `window.isSecureContext` because service workers only register on
// secure origins (https, or localhost). dux is frequently run over plain HTTP on
// a LAN address (e.g. http://192.168.x.x:port), where the SW API is present but
// registration would reject. Gating on isSecureContext keeps the SW dormant
// there — no console errors, no broken-looking UI — instead of attempting a
// register that is guaranteed to fail.
export function registerServiceWorker(): void {
  if (!("serviceWorker" in navigator) || !window.isSecureContext) {
    return;
  }
  // Register after load so the SW install never competes with first paint.
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch(() => {
      // Swallow: offline support is optional, nothing else depends on it.
    });
  });
}
