// Sessions löschen über den Dialog auf der Simulieren-Seite (einzeln und alle).
// Läuft absichtlich zuletzt: löscht ALLE Sessions der Test-Datenbank.
'use strict';

module.exports = {
  name: 'Sessions löschen',
  async run(t) {
    const { sleep } = t.lib;
    await t.api.post('/api/admin/settings', { record_enabled: true, output_rtmp: '' });
    const ids = [];
    for (const name of ['E2E Löschen A', 'E2E Löschen B', 'E2E Löschen C']) {
      const r = await t.api.post('/api/sim/start', { name });
      ids.push(r.json.session_id);
      await sleep(1500);
      await t.api.post('/api/sim/stop', {});
      await t.api.post('/api/admin/settings', { record_enabled: true });
    }
    const list = async () => ((await t.api.get('/api/sessions')).json?.sessions || []).map((s) => s.id);

    const browser = await t.browser();
    const page = await browser.page();
    await t.api.loginPage(page);
    await page.goto(t.base + '/ui/simulate', 3000);
    await page.eval(`window.confirm = () => true; document.getElementById('deleteAllSessionsBtn').click(); 'ok'`);
    await sleep(1500);
    const shown = await page.eval(`[...document.querySelectorAll('.delete-session-checkbox')].map((cb) => Number(cb.dataset.id))`);
    t.check('Dialog listet die Sessions', ids.every((id) => shown.includes(id)), { erwartet: ids, gezeigt: shown });

    await page.eval(`(() => { const cb = document.querySelector('.delete-session-checkbox[data-id="${ids[1]}"]'); cb.checked = true; cb.dispatchEvent(new Event('change')); document.getElementById('deleteSelectedBtn').click(); return 'ok'; })()`);
    await sleep(2500);
    const after1 = await list();
    t.check('Ausgewählte Session gelöscht, andere bleiben', !after1.includes(ids[1]) && after1.includes(ids[0]) && after1.includes(ids[2]), after1);

    await page.eval(`document.getElementById('deleteAllInsideModalBtn').click(); 'ok'`);
    await sleep(3000);
    const after2 = await list();
    t.check('„Alle löschen“ entfernt alle Sessions', after2.length === 0, after2);
    t.check('Aufnahmeordner der Sessions entfernt', ids.every((id) => !require('fs').existsSync(require('path').join(t.dataDir, 'recordings', String(id)))));
    t.check('Keine JS-Fehler', page.errors.length === 0, page.errors);
  },
};
