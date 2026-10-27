// Liveansicht + Aufnahme: Zuschauer beim Start, Zuschauer kommt/geht mittendrin, Stopp.
// Deckt ab: Video fror ein, wenn ein Zuschauer beim Start beitrat; Zuschauer-Zweige
// wurden nie abgebaut (Speicherleck); Stopp hing.
'use strict';

module.exports = {
  name: 'Liveansicht, Aufnahme & Stopp',
  async run(t) {
    const { sleep, PC_HOOK, rtcRates, recordingPackets, waitFor } = t.lib;
    await t.api.post('/api/admin/settings', { record_enabled: true, output_rtmp: '' });

    const browser = await t.browser();
    const live = await browser.page();
    await live.addInitScript(PC_HOOK);
    await t.api.loginPage(live);
    await live.goto(t.base + '/ui/viewer', 2500);
    t.check('Liveansicht ohne Session: „Keine Session“', (await live.eval(`document.getElementById('lvState').textContent`)) === 'Keine Session');

    const mark = t.logMark();
    const start = await t.api.post('/api/sim/start', { name: 'E2E Liveansicht' });
    t.check('Session startet', start.status === 200, start.status);
    const dir = start.json && start.json.recording_folder;

    // Liveansicht verbindet sich selbst, kurz nach dem Start
    const rates = await waitFor(async () => { const r = await rtcRates(live, 1500); return r.videoFps >= 20 ? r : null; }, 20000, 200);
    t.check('Liveansicht zeigt Bild (≥ 20 fps)', !!rates, rates);
    t.check('Badge „Live“', (await live.eval(`document.getElementById('lvState').textContent`)) === 'Live');
    t.check('Auflösung 1920×1080', (await live.eval(`(() => { const v = document.getElementById('lvVideo'); return v.videoWidth + 'x' + v.videoHeight; })()`)) === '1920x1080');

    const p1 = recordingPackets(dir);
    await sleep(3000);
    const p2 = recordingPackets(dir);
    t.check('Aufnahme wächst', p1 > 0 && p2 > p1, { vorher: p1, nachher: p2 });

    // Zweiter Zuschauer kommt und geht wieder (Tab zu) – Aufnahme und erster Zuschauer laufen weiter
    for (let i = 1; i <= 2; i++) {
      const b2 = await t.browser();
      const extra = await b2.page();
      await extra.addInitScript(PC_HOOK);
      await t.api.loginPage(extra);
      await extra.goto(t.base + '/ui/viewer', 3000);
      const r2 = await waitFor(async () => { const r = await rtcRates(extra, 1500); return r.videoFps >= 20 ? r : null; }, 15000, 200);
      t.check(`Zusätzlicher Zuschauer ${i} bekommt Bild`, !!r2, r2);
      b2.close();
      await sleep(2500);
    }
    const p3 = recordingPackets(dir);
    t.check('Aufnahme läuft nach Kommen/Gehen weiter', p3 > p2, { vorher: p2, nachher: p3 });
    const r3 = await rtcRates(live, 1500);
    t.check('Erster Zuschauer weiterhin mit Bild', r3.videoFps >= 20, r3);
    const log = t.logSince(mark);
    const conn = (log.match(/ws\/webrtc: client connected/g) || []).length;
    const disc = (log.match(/ws\/webrtc: client disconnected/g) || []).length;
    t.check('Gegangene Zuschauer werden abgebaut', disc >= 2, { verbunden: conn, abgebaut: disc });

    const t0 = Date.now();
    const stop = await t.api.post('/api/sim/stop', {});
    const ms = Date.now() - t0;
    t.check('Stopp antwortet schnell (< 3 s)', stop.status === 200 && ms < 3000, ms + ' ms');
    const idle = await waitFor(async () => (await live.eval(`document.getElementById('lvState').textContent`)) === 'Keine Session', 10000);
    t.check('Liveansicht nach Stopp: „Keine Session“', !!idle);
    t.check('Keine JS-Fehler', live.errors.length === 0, live.errors);
  },
};
