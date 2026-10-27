// Zugriffsschutz der Live-WebSockets: ohne Login/Ticket kein Bild, keine Durchsage, keine
// Logs; mit Login bzw. Ticket aus show_live/paging_start schon. Streamschlüssel nie im Log.
'use strict';

module.exports = {
  name: 'Zugriffsschutz Live-Verbindungen',
  async run(t) {
    const { sleep, PC_HOOK, rtcRates, START_TX, STOP_TX, waitFor } = t.lib;
    await t.api.post('/api/admin/settings', { record_enabled: false, output_rtmp: 'rtmp://127.0.0.1:1/live/E2E-STREAMKEY-42' });
    await t.api.post('/api/sim/start', { name: 'E2E Zugriff', viewer_ids: [1] });
    await sleep(2500);

    const browser = await t.browser();
    const anon = await browser.page();               // nie eingeloggt
    await anon.addInitScript(PC_HOOK);
    await anon.goto(t.base + '/help/', 1500);
    // Öffnet einen WebSocket und meldet, ob er aufgeht (true) oder abgelehnt wird (false)
    const opens = (path) => anon.eval(`new Promise((res) => { const ws = new WebSocket('ws://' + location.host + ${JSON.stringify(path)});
      ws.onopen = () => { ws.close(); res(true); }; ws.onerror = () => res(false); setTimeout(() => res(null), 4000); })`);

    t.check('Ohne Login: Live-Video abgelehnt', (await opens('/ws/webrtc')) === false);
    t.check('Ohne Login: Durchsage senden abgelehnt', (await opens('/ws/voice/tx?group=default')) === false);
    t.check('Ohne Login: Logs abgelehnt', (await opens('/ws/logs')) === false);
    t.check('Gefälschtes Ticket abgelehnt', (await opens('/ws/webrtc?ticket=abc.def.ghi')) === false);

    // Nicht-Admin darf keine Logs
    await t.api.post('/api/users', { username: 'e2e-user', password: 'e2e-userpass', role: 'user' });
    const user = t.lib.makeApi(t.base);
    let userOk = true;
    try { await user.login('e2e-user', 'e2e-userpass'); } catch { userOk = false; }
    if (userOk) t.check('Nicht-Admin: Logs abgelehnt', (await opens('/ws/logs?token=' + encodeURIComponent(user.token))) === false);
    t.check('Admin: Logs erlaubt', (await opens('/ws/logs?token=' + encodeURIComponent(t.api.token))) === true);

    // Viewer-Bildschirm ohne Login bekommt das Bild über das Ticket aus show_live
    const viewer = await browser.page();
    await viewer.addInitScript(PC_HOOK);
    // Geöffnete WebSocket-Adressen mitschreiben (um das Ticket später erneut zu verwenden)
    await viewer.addInitScript(`window.__wsUrls = []; (() => { const _WS = window.WebSocket;
      window.WebSocket = function (u, p) { window.__wsUrls.push(String(u)); return p ? new _WS(u, p) : new _WS(u); };
      window.WebSocket.prototype = _WS.prototype; Object.assign(window.WebSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 }); })();`);
    await t.api.openPaired(viewer, '/viewer', 1);
    const v = await waitFor(async () => { const r = await rtcRates(viewer, 1500); return r.videoFps >= 20 ? r : null; }, 20000, 200);
    t.check('Viewer ohne Login: Bild über Ticket', !!v, v);

    // Ticket gilt nur für diese Session
    const ticket = await viewer.eval(`(() => { for (const u of window.__wsUrls) { const m = /\\/ws\\/webrtc\\?ticket=([^&]+)/.exec(u); if (m) return decodeURIComponent(m[1]); } return null; })()`);
    const oldTicket = ticket || '';
    await t.api.post('/api/sim/stop', {});
    await t.api.post('/api/sim/start', { name: 'E2E Zugriff 2', viewer_ids: [1] });
    await sleep(2000);
    t.check('Ticket einer beendeten Session gilt nicht mehr', !!oldTicket && (await opens('/ws/webrtc?ticket=' + encodeURIComponent(oldTicket))) === false, oldTicket ? 'geprüft' : 'Ticket nicht auslesbar');

    // Durchsage: eingeloggter Sender, Viewer hört über Ticket aus paging_start
    const tx = await browser.page();
    await t.api.loginPage(tx);
    await tx.eval(START_TX('default', true));
    const heard = await waitFor(async () => { const r = await rtcRates(viewer, 2000); return r.pagingBytesPerSec > 1000 ? r : null; }, 15000, 200);
    t.check('Eingeloggt: Durchsage geht, Viewer hört über Ticket mit', !!heard, heard);
    t.check('Ohne Login: Durchsage mithören abgelehnt', (await opens('/ws/voice/viewer')) === false);
    await tx.eval(STOP_TX);

    t.check('Streamschlüssel steht nicht im Serverlog', !t.serverLog().includes('E2E-STREAMKEY-42'));
    t.check('Keine JS-Fehler', viewer.errors.length + tx.errors.length === 0, [...viewer.errors, ...tx.errors]);
  },
};
