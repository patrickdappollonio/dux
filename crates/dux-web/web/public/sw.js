// dux service worker: offline fallback only, caching nothing but /offline.html.
//
// There is no app-shell caching and no precaching of the JS/CSS bundle: dux
// ships a bundle embedded in the Rust binary, and a cached bundle running
// against a live WebSocket protocol is stale with nothing to say so. The app is
// always loaded from the network, and the offline page exists only so a
// navigation does not dead-end on the browser's raw error screen.

// Bump this version to invalidate the cached offline page (e.g. when offline.html
// changes). Old versioned caches are pruned in `activate`.
const CACHE = "dux-offline-v3";
const OFFLINE_URL = "/offline.html";

self.addEventListener("install", (event) => {
  event.waitUntil(
    // `cache: "reload"` bypasses the HTTP cache so a stale browser-cached copy
    // of offline.html can never be what we store here.
    caches
      .open(CACHE)
      .then((cache) => cache.add(new Request(OFFLINE_URL, { cache: "reload" }))),
  );
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
  // Navigations only: everything else (the bundle, the WebSocket upgrade, icons,
  // the manifest) never reaches respondWith, which is what keeps it uncached.
  if (event.request.mode !== "navigate") {
    return;
  }
  event.respondWith(
    fetch(event.request).catch(() =>
      // A partially failed install can leave the offline page uncached; a
      // network-error response beats respondWith(undefined) throwing and
      // dead-ending the navigation.
      caches.match(OFFLINE_URL).then((cached) => cached ?? Response.error()),
    ),
  );
});
