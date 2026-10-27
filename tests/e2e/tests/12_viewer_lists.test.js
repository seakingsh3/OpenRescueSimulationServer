// Viewer-Listen: Einstellungen laden selbst, Viewer-Dialog auf der Einstellungsseite wird
// gefüllt (früher doppelte ID), Start-Dialog zeigt neu verbundene Viewer und neue Räume,
// getrennte Viewer gelten sofort als offline (früher dauerhaft „online“).
'use strict';

module.exports = {
  name: 'Viewer-Listen & Online-Status',
  async run(t) {
    const { sleep, waitFor } = t.lib;
    for (const n of [21, 22]) await t.api.post('/api/viewers', { name: `E2E Viewer ${n}`, url: `/viewer/${n}`, alias: `E2E Bildschirm ${n}` });
    const online = async () => ((await t.api.get('/api/viewers')).json?.viewers || []).filter((v) => v.online).map((v) => v.id);
    const idOf = async (n) => ((await t.api.get('/api/viewers')).json.viewers.find((v) => v.url === `/viewer/${n}`) || {}).id;

    // Online/Offline
    const vb = await t.browser();
    const v21 = await vb.page();
    await t.api.openPaired(v21, '/viewer', 21);
    const id21 = await idOf(21);
    t.check('Verbundener Viewer ist online', (await online()).includes(id21), await online());
    vb.close();
    const off = await waitFor(async () => !(await online()).includes(id21), 8000);
    t.check('Nach Schließen sofort offline', !!off, await online());

    // Einstellungen + Dialog auf der Einstellungsseite
    const browser = await t.browser();
    const page = await browser.page();
    await t.api.loginPage(page);
    await page.goto(t.base + '/ui/options', 2500);
    const settingsText = () => page.eval(`document.getElementById('settingsViewerList')?.innerText || ''`);
    t.check('Einstellungen laden die Viewer-Liste selbst', (await settingsText()).includes('E2E Bildschirm 21'));
    await page.eval(`document.getElementById('viewerBtn').click(); 'ok'`);
    await sleep(1500);
    const dialog = await page.eval(`document.querySelector('#viewerModal #viewerList').innerText`);
    t.check('Viewer-Dialog auf der Einstellungsseite gefüllt', dialog.includes('E2E Bildschirm 21') && !dialog.includes('Lade Viewer'), dialog.slice(0, 80));
    await page.eval(`document.getElementById('closeViewerModal').click(); 'ok'`);

    // Start-Dialog: zuerst ohne Viewer 22, dann kommt er online und ein Raum wird angelegt
    await page.goto(t.base + '/ui/simulate', 2500);
    const startDialog = async () => {
      await page.eval(`document.getElementById('startBtn').click(); 'ok'`);
      await sleep(2000);
      const r = await page.eval(`({ viewer: document.getElementById('viewerSelectList').innerText, raeume: [...document.querySelectorAll('#roomSelect option')].map((o) => o.textContent).join(' | ') })`);
      await page.eval(`document.getElementById('cancelSessionNameBtn').click(); 'ok'`);
      return r;
    };
    const first = await startDialog();
    t.check('Start-Dialog: Viewer 22 anfangs nicht dabei', !first.viewer.includes('E2E Bildschirm 22'));
    const vb2 = await t.browser();
    await t.api.openPaired(await vb2.page(), '/viewer', 22);
    await t.api.post('/api/rooms', { name: 'E2E Neuer Raum', layout: '1', sources: '[]' });
    const second = await startDialog();
    t.check('Start-Dialog: neu verbundener Viewer erscheint', second.viewer.includes('E2E Bildschirm 22'), second.viewer.slice(0, 120));
    t.check('Start-Dialog: neu angelegter Raum erscheint', second.raeume.includes('E2E Neuer Raum'), second.raeume);
    vb2.close();
    t.check('Keine JS-Fehler', page.errors.length === 0, page.errors);
  },
};
