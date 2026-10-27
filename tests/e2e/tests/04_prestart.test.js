// Vor-Start: Abbruch über alle Wege, Freigabe, Neustart; Oberfläche (Liveansicht-Badge,
// Freigeben-Knopf nach Neuladen). Deckt ab: abgebrochener Vor-Start sperrte jeden weiteren.
'use strict';

module.exports = {
  name: 'Vor-Start',
  async run(t) {
    const { sleep, waitFor } = t.lib;
    const status = async () => (await fetch(t.base + '/api/sim/status').then((r) => r.json()));
    const sessionNames = async () => {
      const r = await t.api.get('/api/sessions');
      return (r.json?.sessions || r.json || []).map((s) => s.name);
    };
    const pre = async (name) => (await t.api.post('/api/sim/prestart', { name }));
    await t.api.post('/api/admin/settings', { record_enabled: false, output_rtmp: '' });

    let r = await pre('E2E VS Abbruch');
    t.check('Vor-Start startet', r.status === 200, r.status);
    t.check('Status meldet Vor-Start', (await status()).prestart_session_id === r.json.session_id);
    r = await t.api.post('/api/sim/stop_all', {});
    t.check('Stopp (alle) bricht Vor-Start ab', r.json?.cancelled_prestart != null && (await status()).prestart_session_id == null, r.json);
    r = await pre('E2E VS nach Abbruch');
    t.check('Danach ist wieder ein Vor-Start möglich', r.status === 200, r.json);
    r = await t.api.post('/api/sim/stop', {});
    t.check('Einzel-Stopp bricht Vor-Start ab', r.status === 200 && (await status()).prestart_session_id == null, r.status);

    await pre('E2E VS alt');
    r = await pre('E2E VS neu');
    t.check('Neuer Vor-Start ersetzt alten', r.status === 200 && (await status()).prestart_session_id === r.json.session_id);
    r = await t.api.post('/api/sim/start', { name: 'E2E Direktstart' });
    let st = await status();
    t.check('Direkter Start ersetzt Vor-Start', r.status === 200 && st.active && st.prestart_session_id == null, st);
    await t.api.post('/api/sim/stop', {});

    r = await pre('E2E VS Freigabe');
    const commit = await t.api.post('/api/sim/commit', { name: 'E2E VS Freigabe' });
    st = await status();
    t.check('Freigabe aktiviert die Vor-Start-Session', commit.status === 200 && st.active && st.session_id === r.json.session_id, st);
    await t.api.post('/api/sim/stop', {});

    await pre('E2E VS Neustart');
    await t.restartServer();
    await t.api.login();
    t.check('Serverneustart räumt Vor-Start ab', (await status()).prestart_session_id == null);

    const names = await sessionNames();
    const leftovers = names.filter((n) => /E2E VS (Abbruch|nach Abbruch|alt|neu|Neustart)/.test(n || ''));
    t.check('Abgebrochene Vor-Starts hinterlassen keine Sessions', leftovers.length === 0, leftovers);
    t.check('Echte Sessions bleiben erhalten', names.includes('E2E Direktstart') && names.includes('E2E VS Freigabe'));

    // Oberfläche
    const browser = await t.browser();
    const live = await browser.page();
    const sim = await browser.page();
    await t.api.loginPage(live);
    await live.eval(`localStorage.setItem('ors_last_start_payload_v1', JSON.stringify({ name: 'E2E UI', viewer_ids: [], viewer_map: [], room_id: null })); 'ok'`);
    await live.goto(t.base + '/ui/viewer', 2000);
    await sim.goto(t.base + '/ui/simulate', 2500);
    await pre('E2E UI');
    t.check('Liveansicht zeigt „Vor-Start“', !!(await waitFor(async () => (await live.eval(`document.getElementById('lvState').textContent`)) === 'Vor-Start', 10000)));
    await sim.reload(3500);
    t.check('Freigeben-Knopf nach Neuladen sichtbar', await sim.eval(`!document.getElementById('commitBtnWrapper').classList.contains('hidden')`));
    await sim.eval(`document.getElementById('stopBtn').click(); 'ok'`);
    await sleep(4000);
    t.check('Nach Stopp: Freigeben-Knopf weg', await sim.eval(`document.getElementById('commitBtnWrapper').classList.contains('hidden')`));
    t.check('Nach Stopp: Liveansicht „Keine Session“', !!(await waitFor(async () => (await live.eval(`document.getElementById('lvState').textContent`)) === 'Keine Session', 10000)));
    t.check('Keine JS-Fehler', live.errors.length + sim.errors.length === 0, [...live.errors, ...sim.errors]);
  },
};
