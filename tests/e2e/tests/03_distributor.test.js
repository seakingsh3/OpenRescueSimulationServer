// Externer Verteiler (RTMP): unerreichbar, erreichbar (Bild + Ton), Ausfall mittendrin.
// Deckt ab: unerreichbarer Verteiler legte Aufnahme/Livebild lahm und blockierte den Stopp.
'use strict';
const { spawn } = require('child_process');
const path = require('path');

module.exports = {
  name: 'Externer Verteiler (RTMP)',
  needs: ['ffmpeg'],
  async run(t) {
    const { sleep, PC_HOOK, rtcRates, recordingPackets, probeStreams, waitFor } = t.lib;
    const port = Number(process.env.ORS_E2E_RTMP_PORT || 19361);
    const url = `rtmp://127.0.0.1:${port}/live/e2e-geheim`;
    const status = async () => (await t.api.get('/api/host/stats')).json?.rtmp || {};

    const browser = await t.browser();
    const live = await browser.page();
    await live.addInitScript(PC_HOOK);
    await t.api.loginPage(live);
    await live.goto(t.base + '/ui/viewer', 2500);
    await live.eval(`window.__toast = false; new MutationObserver(() => {
      if (document.body.textContent.includes('Externer Verteiler nicht erreichbar')) window.__toast = true;
    }).observe(document.body, { childList: true, subtree: true }); 'ok'`);

    // 1) Verteiler nicht erreichbar
    await t.api.post('/api/admin/settings', { record_enabled: true, output_rtmp: url });
    const s1 = await t.api.post('/api/sim/start', { name: 'E2E Verteiler weg' });
    const dir1 = s1.json && s1.json.recording_folder;
    await sleep(6000);
    const p1 = recordingPackets(dir1);
    await sleep(3000);
    const p2 = recordingPackets(dir1);
    t.check('Unerreichbar: Aufnahme läuft', p1 > 0 && p2 > p1, { p1, p2 });
    const r1 = await rtcRates(live, 1500);
    t.check('Unerreichbar: Liveansicht mit Bild', r1.videoFps >= 20, r1);
    const st1 = await status();
    t.check('Unerreichbar: Status „error“, Streamkey maskiert', st1.state === 'error' && !String(st1.url).includes('e2e-geheim'), st1);
    t.check('Unerreichbar: Hinweis „Externer Verteiler nicht erreichbar“', await live.eval('window.__toast'));
    let t0 = Date.now();
    const stop1 = await t.api.post('/api/sim/stop', {});
    t.check('Unerreichbar: Stopp schnell', stop1.status === 200 && Date.now() - t0 < 3000, (Date.now() - t0) + ' ms');
    await sleep(1500);

    // 2) Verteiler erreichbar (ffmpeg als RTMP-Empfänger): Bild und Ton kommen an
    const flv = path.join(t.workDir, 'rtmp_empfang.flv');
    const ff = spawn('ffmpeg', ['-loglevel', 'error', '-listen', '1', '-i', url, '-c', 'copy', '-y', flv], { stdio: 'ignore' });
    await sleep(1000);
    await t.api.post('/api/sim/start', { name: 'E2E Verteiler da' });
    const streaming = await waitFor(async () => (await status()).state === 'streaming', 15000);
    t.check('Erreichbar: Status „streaming“', !!streaming, await status());
    await sleep(10000);
    await t.api.post('/api/sim/stop', {});
    await Promise.race([new Promise((r) => ff.once('exit', r)), sleep(8000)]);
    try { ff.kill('SIGKILL'); } catch {}
    const streams = probeStreams(flv);
    const v = streams.find((s) => s.codec_type === 'video') || {};
    const a = streams.find((s) => s.codec_type === 'audio') || {};
    t.check('Erreichbar: Bild H.264 1920×1080 kommt an', v.codec_name === 'h264' && v.width === 1920 && Number(v.nb_read_packets) > 100, v);
    t.check('Erreichbar: Ton AAC kommt an', a.codec_name === 'aac' && Number(a.nb_read_packets) > 100, a);
    await sleep(1500);

    // 3) Verteiler fällt mittendrin weg
    await live.eval(`window.__toast = false; 'ok'`);
    const ff2 = spawn('ffmpeg', ['-loglevel', 'error', '-listen', '1', '-i', url, '-c', 'copy', '-y', path.join(t.workDir, 'rtmp2.flv')], { stdio: 'ignore' });
    await sleep(1000);
    // sim_stop schaltet die Aufnahme nach jeder Session ab (gewollt) → wieder einschalten
    await t.api.post('/api/admin/settings', { record_enabled: true, output_rtmp: url });
    const s3 = await t.api.post('/api/sim/start', { name: 'E2E Verteiler fällt aus' });
    const dir3 = s3.json && s3.json.recording_folder;
    await waitFor(async () => (await status()).state === 'streaming', 15000);
    ff2.kill('SIGKILL');
    const failed = await waitFor(async () => (await status()).state === 'error', 15000);
    t.check('Ausfall mittendrin: Status „error“', !!failed, await status());
    await waitFor(() => recordingPackets(dir3) > 0, 15000);
    const q1 = recordingPackets(dir3);
    await sleep(3000);
    const q2 = recordingPackets(dir3);
    t.check('Ausfall mittendrin: Aufnahme läuft weiter', q2 > q1 && q1 > 0, { q1, q2 });
    const r3 = await rtcRates(live, 1500);
    t.check('Ausfall mittendrin: Liveansicht mit Bild', r3.videoFps >= 20, r3);
    t.check('Ausfall mittendrin: Hinweis erscheint', await waitFor(() => live.eval('window.__toast'), 5000));
    t0 = Date.now();
    await t.api.post('/api/sim/stop', {});
    t.check('Ausfall mittendrin: Stopp schnell', Date.now() - t0 < 3000, (Date.now() - t0) + ' ms');
    t.check('Keine JS-Fehler', live.errors.length === 0, live.errors);
  },
};
