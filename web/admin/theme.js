/* Hell/Dunkel wie auf der Hören-Seite, vor dem Zeichnen (eigene Datei wegen der strengen CSP) */
(function () {
  var t = null; try { t = localStorage.getItem('crab_theme'); } catch (e) {}
  var h = document.documentElement; if (t === 'dark' || t === 'light') h.setAttribute('data-theme', t);
  function sys() { if (!h.getAttribute('data-theme')) h.classList.toggle('sysdark', window.matchMedia('(prefers-color-scheme: dark)').matches); }
  sys(); window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', sys);
})();
