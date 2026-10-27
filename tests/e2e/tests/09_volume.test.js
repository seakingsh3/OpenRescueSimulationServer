// Stream-Lautstärke (Regler auf der Simulieren-Seite) wirkt auf Zuschauer – auch auf solche,
// die erst danach beitreten. Server läuft dafür mit Testton (ORS_AUDIO_TESTSRC=1).
'use strict';

module.exports = {
  name: 'Stream-Lautstärke für Zuschauer',
  async run(t) {
    const { sleep, PC_HOOK, waitFor } = t.lib;
    await t.restartServer({ ORS_AUDIO_TESTSRC: '1' });
    await t.api.login();
    await t.api.post('/api/audio/volume/set', { volumePercent: 100 });
    const state0 = (await t.api.get('/api/audio/state')).json || {};
    if (state0.muted) await t.api.post('/api/audio/mute/toggle', {});
    await t.api.post('/api/admin/settings', { record_enabled: false, output_rtmp: '' });

    // Mittlerer Audiopegel (0..1) des Live-Tons über ~1,5 s
    const LEVEL = `(async () => { let sum = 0, n = 0;
      for (let i = 0; i < 6; i++) {
        for (const pc of window.__pcs) { if (pc.connectionState !== 'connected') continue;
          const kinds = pc.getReceivers().map((r) => r.track && r.track.kind);
          if (!kinds.includes('video')) continue;
          (await pc.getStats()).forEach((x) => { if (x.type === 'inbound-rtp' && x.kind === 'audio' && typeof x.audioLevel === 'number') { sum += x.audioLevel; n++; } }); }
        await new Promise((r) => setTimeout(r, 250)); }
      return n ? sum / n : -1; })()`;
    const openViewer = async () => {
      const b = await t.browser();
      const page = await b.page();
      await page.addInitScript(PC_HOOK);
      await t.api.loginPage(page);
      await page.goto(t.base + '/ui/viewer', 2500);
      await page.eval(`document.getElementById('lvVideo').muted = false; 'ok'`);
      return { b, page };
    };

    try {
      const { page } = await openViewer();
      await t.api.post('/api/sim/start', { name: 'E2E Lautstärke' });
      const loud = await waitFor(async () => { const l = await page.eval(LEVEL); return l > 0.01 ? l : null; }, 20000, 200);
      t.check('Bei 100 % ist der Testton hörbar', !!loud, loud);

      await t.api.post('/api/audio/volume/set', { volumePercent: 0 });
      await sleep(1500);
      const quiet = await page.eval(LEVEL);
      t.check('Regler auf 0 % → Zuschauer still', quiet >= 0 && quiet < 0.003, { vorher: loud, jetzt: quiet });

      const { page: late } = await openViewer();
      await waitFor(async () => (await late.eval(`window.__pcs.some((pc) => pc.connectionState === 'connected')`)), 15000);
      await sleep(1500);
      const lateLevel = await late.eval(LEVEL);
      t.check('Danach beigetretener Zuschauer übernimmt 0 %', lateLevel >= 0 && lateLevel < 0.003, lateLevel);

      await t.api.post('/api/audio/volume/set', { volumePercent: 100 });
      await sleep(1500);
      const back = await page.eval(LEVEL);
      const backLate = await late.eval(LEVEL);
      t.check('Zurück auf 100 % → beide wieder hörbar', back > 0.01 && backLate > 0.01, { erster: back, zweiter: backLate });

      await t.api.post('/api/audio/mute/toggle', {});
      await sleep(1500);
      const muted = await page.eval(LEVEL);
      t.check('Stumm → Zuschauer still', muted >= 0 && muted < 0.003, muted);
      await t.api.post('/api/audio/mute/toggle', {});
    } finally {
      await t.api.post('/api/sim/stop', {});
      await t.restartServer();
    }
  },
};
