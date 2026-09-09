// Service-worker registration for dux's offline-fallback PWA support: pure progressive
// enhancement, so every failure is swallowed silently. Gated on `window.isSecureContext`
// because service workers register only on secure origins, and dux is frequently served over
// plain HTTP on a LAN address, where the API is present but registration would reject.
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
