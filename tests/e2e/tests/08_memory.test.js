// Speicher über viele Sessions mit Zuschauer (langsam, nur mit --slow).
// Deckt ab: jeder Zuschauer hinterließ seinen Zweig; RSS wuchs um ~80 MB pro Session.
'use strict';

module.exports = {
  name: 'Speicher über 10 Sessions',
  slow: true,
  async run(t) {
    const { sleep, PC_HOOK, waitFor, rtcRates, serverRss } = t.lib;
    await t.api.post('/api/admin/settings', { record_enabled: true, output_rtmp: '' });
    const threads = () => { try { return require('fs').readdirSync(`/proc/${t.serverPid}/task`).length; } catch { return -1; } };
    const mark = t.logMark();
    const samples = [];
    for (let i = 1; i <= 10; i++) {
      const browser = await t.browser();
      const live = await browser.page();
      await live.addInitScript(PC_HOOK);
      await t.api.loginPage(live);
      await live.goto(t.base + '/ui/viewer', 2000);
      await t.api.post('/api/sim/start', { name: `E2E Speicher ${i}` });
      await waitFor(async () => (await rtcRates(live, 1000)).videoFps >= 20, 15000, 200);
      await sleep(4000);
      await t.api.post('/api/sim/stop', {});
      browser.close();
      await sleep(5000); // Aufräumen + malloc_trim (3 s nach dem Stopp)
      samples.push({ session: i, rssMb: serverRss(t.serverPid), threads: threads() });
    }
    console.log('   Verlauf:', samples.map((s) => `${s.session}:${s.rssMb}MB/${s.threads}T`).join('  '));
    const log = t.logSince(mark);
    const conn = (log.match(/ws\/webrtc: client connected/g) || []).length;
    const disc = (log.match(/ws\/webrtc: client disconnected/g) || []).length;
    t.check('Alle Zuschauer-Zweige werden abgebaut', conn > 0 && conn === disc, { verbunden: conn, abgebaut: disc });
    const growth = samples[9].rssMb - samples[2].rssMb;
    t.check('Speicher wächst ab Session 3 um weniger als 150 MB', growth < 150, growth + ' MB');
    t.check('Thread-Zahl bleibt stabil (±6)', Math.abs(samples[9].threads - samples[2].threads) <= 6, { s3: samples[2].threads, s10: samples[9].threads });
    t.check('Kein Absturz', samples.every((s) => s.rssMb > 0));
  },
};
