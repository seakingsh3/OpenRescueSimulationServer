// === options_tabs.js ================================================
// Reiter auf der Einstellungen-Seite. Alle Bereiche bleiben im DOM (options.js
// arbeitet weiter mit denselben IDs), es wird nur ein Bereich eingeblendet.
// Auswahl: URL-Hash (#tab=kameras) > zuletzt gewählter Reiter > erster Reiter.
(function () {
  'use strict';
  const STORAGE_KEY = 'ors_options_tab';
  const tabs = Array.from(document.querySelectorAll('#optionsTabs [data-tab]'));
  const panels = Array.from(document.querySelectorAll('[data-tab-panel]'));
  if (!tabs.length || !panels.length) return;

  function tabFromHash() {
    const m = /(?:^|[#&])tab=([\w-]+)/.exec(window.location.hash || '');
    return m ? m[1] : null;
  }

  function activate(name, { updateHash = true } = {}) {
    if (!tabs.some((t) => t.dataset.tab === name)) name = tabs[0].dataset.tab;
    tabs.forEach((t) => {
      const on = t.dataset.tab === name;
      t.classList.toggle('tab-active', on);
      t.setAttribute('aria-selected', on ? 'true' : 'false');
    });
    panels.forEach((p) => p.classList.toggle('hidden', p.dataset.tabPanel !== name));
    try { localStorage.setItem(STORAGE_KEY, name); } catch {}
    if (updateHash) {
      try { history.replaceState(null, '', `#tab=${name}`); } catch {}
    }
  }

  tabs.forEach((t) => {
    t.addEventListener('click', (e) => {
      e.preventDefault();
      activate(t.dataset.tab);
    });
  });
  window.addEventListener('hashchange', () => {
    const h = tabFromHash();
    if (h) activate(h, { updateHash: false });
  });

  let initial = tabFromHash();
  if (!initial) {
    try { initial = localStorage.getItem(STORAGE_KEY); } catch {}
  }
  activate(initial || tabs[0].dataset.tab, { updateHash: !!tabFromHash() });
})();
