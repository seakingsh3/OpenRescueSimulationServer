// Kopplung von Viewer-Bildschirmen und Pagern: ungekoppelt kein Bild/keine Durchsage,
// Koppeln per PIN (aus den Einstellungen) und per QR-Link, Entkoppeln, Schutz gegen Raten.
'use strict';

module.exports = {
  name: 'Kopplung Viewer & Pager',
  async run(t) {
    const { sleep, PC_HOOK, rtcRates, START_TX, STOP_TX, waitFor } = t.lib;
    await t.api.post('/api/admin/settings', { record_enabled: false, output_rtmp: '' });
    const onlineNos = async () => ((await t.api.get('/api/viewers')).json?.viewers || []).filter((v) => v.online).map((v) => v.url);

    const browser = await t.browser();
    const viewer = await browser.page();          // nie eingeloggt, nie gekoppelt
    await viewer.addInitScript(PC_HOOK);
    await viewer.goto(t.base + '/viewer/31', 3000);
    await t.api.post('/api/sim/start', { name: 'E2E Kopplung', viewer_ids: [] });
    await sleep(2500);
    const overlay = () => viewer.eval(`!!document.getElementById('orsPairOverlay')`);
    t.check('Ungekoppelt: „Bitte koppeln“ erscheint', await overlay());
    t.check('Ungekoppelt: gilt nicht als online', !(await onlineNos()).includes('/viewer/31'), await onlineNos());
    t.check('Ungekoppelt: kein Bild', (await rtcRates(viewer, 1500)).videoFps === 0);

    // PIN aus den Einstellungen
    const admin = await browser.page();
    await t.api.loginPage(admin);
    await admin.goto(t.base + '/ui/options', 3000);
    await admin.eval(`document.getElementById('pairDeviceBtn').click(); 'ok'`);
    const pin = await waitFor(async () => { const p = await admin.eval(`document.getElementById('pairPin').textContent.trim()`); return /^\d{6}$/.test(p) ? p : null; }, 8000);
    t.check('Einstellungen zeigen eine 6-stellige PIN', !!pin, pin);

    // falsche PIN wird abgelehnt
    await viewer.eval(`(() => { const i = document.getElementById('orsPairPin'); i.value = '${pin === '000000' ? '111111' : '000000'}'; document.getElementById('orsPairOk').click(); return 'ok'; })()`);
    await sleep(1500);
    t.check('Falsche PIN abgelehnt', await overlay() && /ungültig/.test(await viewer.eval(`document.getElementById('orsPairMsg').textContent`)));

    // richtige PIN
    await viewer.eval(`(() => { const i = document.getElementById('orsPairPin'); i.value = '${pin}'; document.getElementById('orsPairOk').click(); return 'ok'; })()`);
    const paired = await waitFor(async () => !(await overlay()), 8000);
    t.check('Richtige PIN koppelt, Maske verschwindet', !!paired);
    const confirmed = await waitFor(async () => /gekoppelt/.test(await admin.eval(`document.getElementById('pairResult').textContent`)), 6000);
    t.check('Einstellungen melden „gekoppelt“', !!confirmed);
    t.check('Gekoppelt: jetzt online', !!(await waitFor(async () => (await onlineNos()).includes('/viewer/31'), 8000)));

    // PIN ist verbraucht
    const reuse = await t.api.post('/api/pairing/claim', { code: pin, viewer_no: 32 });
    t.check('PIN gilt nur einmal', reuse.status === 403, reuse.status);

    // Gekoppelter Viewer bekommt Bild, wenn eine Session für ihn startet
    await t.api.post('/api/sim/stop', {});
    const db31 = ((await t.api.get('/api/viewers')).json.viewers.find((v) => v.url === '/viewer/31') || {}).id;
    await t.api.post('/api/sim/start', { name: 'E2E Kopplung 2', viewer_ids: [db31] });
    const v = await waitFor(async () => { const r = await rtcRates(viewer, 1500); return r.videoFps >= 20 ? r : null; }, 20000, 200);
    t.check('Gekoppelt: Bild nach Session-Start', !!v, v);
    await viewer.reload(3500);
    t.check('Kopplung übersteht Neuladen', !(await overlay()) && (await onlineNos()).includes('/viewer/31'));

    // Pager per QR-Link koppeln und Durchsage hören
    const pager = await browser.page();
    await pager.addInitScript(PC_HOOK);
    await t.api.openPaired(pager, '/pager', 33);
    t.check('Pager per QR-Link gekoppelt (keine Maske)', !(await pager.eval(`!!document.getElementById('orsPairOverlay')`)));
    const tx = await browser.page();
    await t.api.loginPage(tx);
    await tx.eval(START_TX('default', true));
    const heard = await waitFor(async () => { const r = await rtcRates(pager, 2000); return r.pagingBytesPerSec > 1000 ? r : null; }, 15000, 200);
    t.check('Gekoppelter Pager hört die Durchsage', !!heard, heard);
    await tx.eval(STOP_TX);

    // QR-Code ist an seine Nummer gebunden
    const qr = await t.api.post('/api/pairing/start', { viewer_no: 34 });
    const wrongNo = await t.api.post('/api/pairing/claim', { code: qr.json.code, viewer_no: 35 });
    t.check('QR-Code gilt nur für seine Nummer', wrongNo.status === 403, wrongNo.status);

    // Entkoppeln über die Einstellungen → Viewer verliert Bild und fragt wieder
    await admin.eval(`document.getElementById('closePairModalBtn').click(); 'ok'`);
    await admin.reload(3000);
    await admin.eval(`window.confirm = () => true; [...document.querySelectorAll('#pairingsList [data-unpair]')].find((b) => b.dataset.no === '31').click(); 'ok'`);
    const askedAgain = await waitFor(() => overlay(), 8000);
    t.check('Entkoppeln: Viewer zeigt wieder „Bitte koppeln“', !!askedAgain);
    await sleep(1500);
    t.check('Entkoppeln: Bild weg', (await rtcRates(viewer, 1500)).videoFps === 0);
    t.check('Entkoppeln: offline', !(await onlineNos()).includes('/viewer/31'));

    // Raten: nach 10 Fehlversuchen werden offene PINs verworfen
    const p2 = (await t.api.post('/api/pairing/start', {})).json.pin;
    for (let i = 0; i < 10; i++) await t.api.post('/api/pairing/claim', { code: String((Number(p2) + 1 + i) % 1000000).padStart(6, '0'), viewer_no: 36 });
    const late = await t.api.post('/api/pairing/claim', { code: p2, viewer_no: 36 });
    t.check('Nach 10 falschen PINs ist auch die richtige verworfen', late.status === 403, late.status);

    t.check('Ungekoppelte Verbindung legt keinen Viewer an', !((await t.api.get('/api/viewers')).json.viewers.some((x) => x.url === '/viewer/36')));
    t.check('Keine JS-Fehler', viewer.errors.length + admin.errors.length + pager.errors.length === 0, [...viewer.errors, ...admin.errors, ...pager.errors]);
  },
};
