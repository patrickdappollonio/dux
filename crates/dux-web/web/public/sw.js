// dux service worker — offline-fallback ONLY.
//
// ==========================================================================
//  DELIBERATE DESIGN: this SW caches NOTHING but /offline.html.
// ==========================================================================
//  There is intentionally NO app-shell caching and NO precaching of the JS/CSS
//  bundle. dux ships a freshly-built bundle embedded in the Rust binary, and the
//  whole point of this SW is to guarantee ZERO stale-bundle risk: the app is
//  always loaded straight from the network. The only thing we keep offline is a
//  small branded "server unreachable" page so navigations don't dead-end on the
//  browser's raw error screen.
//
//  If you are a future contributor tempted to "improve" this by caching assets,
//  DON'T. Stale bundles against a live WebSocket protocol are a debugging
//  nightmare. The offline page is the entire contract.
// ==========================================================================

// Bump this version to invalidate the cached offline page (e.g. when offline.html
// changes). Old versioned caches are pruned in `activate`.
const CACHE = "dux-offline-v1";
const OFFLINE_URL = "/offline.html";

self.addEventListener("install", (event) => {
  event.waitUntil(
    // `cache: "reload"` bypasses the HTTP cache so a stale browser-cached copy
    // of offline.html can never be what we store here.
    caches
      .open(CACHE)
      .then((cache) => cache.add(new Request(OFFLINE_URL, { cache: "reload" }))),
  );
  // Take over without waiting for existing tabs to close.
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  // Only intercept top-level navigations. Everything else (the bundle, the
  // WebSocket upgrade, icons, the manifest) is left entirely to the browser —
  // we never call respondWith for those, so there is no caching of any kind.
  if (event.request.mode !== "navigate") {
    return;
  }
  event.respondWith(
    fetch(event.request).catch(() =>
      // If the offline page somehow isn't cached (e.g. a partially failed
      // install), fall back to a network-error response rather than letting
      // respondWith(undefined) throw and dead-end the navigation.
      caches.match(OFFLINE_URL).then((cached) => cached ?? Response.error()),
    ),
  );
});
