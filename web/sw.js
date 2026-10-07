/* Service Worker: nur für die Installierbarkeit als Web-App. Nichts wird gecacht,
   der WebSDR-Stream muss immer live vom Server kommen. */
self.addEventListener('install', function () { self.skipWaiting(); });
self.addEventListener('activate', function (e) { e.waitUntil(self.clients.claim()); });
self.addEventListener('fetch', function () { /* Netzwerk wie gehabt */ });
