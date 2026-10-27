// Durchsage an Viewer: eigener Ton-Kanal parallel zum Live-Video.
// Deckt ab: Chrome verwarf das Opus-Angebot (fehlende Kanalangabe); show_live beendete die
// Durchsage; Viewer, die sich während einer Durchsage verbinden, hörten nichts.
'use strict';

module.exports = {
  name: 'Durchsage an Viewer',
  async run(t) {
    const { sleep, PC_HOOK, rtcRates, START_TX, STOP_TX, waitFor } = t.lib;
    await t.api.post('/api/admin/settings', { record_enabled: false, output_rtmp: '' });

    const browser = await t.browser();
    const viewer = await browser.page();
    await viewer.addInitScript(PC_HOOK);
    await t.api.openPaired(viewer, '/viewer', 1);
    const pager = await browser.page();
    await pager.addInitScript(PC_HOOK);
    await t.api.openPaired(pager, '/pager', 77);
    const tx = await browser.page();
    await t.api.loginPage(tx);

    await t.api.post('/api/sim/start', { name: 'E2E Durchsage Viewer', viewer_ids: [1] });
    const video = await waitFor(async () => { const r = await rtcRates(viewer, 1500); return r.videoFps >= 20 ? r : null; }, 20000, 200);
    t.check('Viewer zeigt Live-Video', !!video, video);

    await tx.eval(START_TX('default', true));
    const during = await waitFor(async () => { const r = await rtcRates(viewer, 2000); return r.pagingBytesPerSec > 1000 ? r : null; }, 15000, 200);
    t.check('Viewer empfängt Durchsage-Ton', !!during, during);
    t.check('Video läuft während der Durchsage weiter', during && during.videoFps >= 20, during);
    const pg = await waitFor(async () => { const r = await rtcRates(pager, 2000); return r.pagingBytesPerSec > 1000 ? r : null; }, 15000, 200);
    t.check('Pager-Seite empfängt die Durchsage', !!pg, pg);

    await viewer.reload(3000);
    const after = await waitFor(async () => { const r = await rtcRates(viewer, 2000); return r.pagingBytesPerSec > 1000 && r.videoFps >= 20 ? r : null; }, 15000, 200);
    t.check('Viewer neu geladen während Durchsage: hört sofort mit', !!after, after || await rtcRates(viewer, 1500));

    await tx.eval(STOP_TX);
    await sleep(3000);
    const end = await rtcRates(viewer, 1500);
    t.check('Nach der Durchsage: kein Ton-Kanal mehr, Video läuft', end.pagingPcs === 0 && end.videoFps >= 20, end);
    t.check('Keine JS-Fehler', viewer.errors.length + tx.errors.length + pager.errors.length === 0, [...viewer.errors, ...tx.errors, ...pager.errors]);
  },
};
