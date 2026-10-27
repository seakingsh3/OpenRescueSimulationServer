// Gemeinsame Helfer für die Ende-zu-Ende-Tests (siehe README.md).
// Keine npm-Abhängigkeiten: Chrome wird per DevTools-Protokoll über eine Pipe gesteuert.
'use strict';
const { spawn, execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------- Chrome / CDP
async function launchBrowser(workDir, extraArgs = []) {
  const chrome = spawn(process.env.CHROME_BIN || 'google-chrome', [
    '--headless=new', '--disable-gpu', '--no-sandbox', '--no-first-run', '--no-default-browser-check',
    '--autoplay-policy=no-user-gesture-required',
    '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
    '--window-size=1400,1000', '--remote-debugging-pipe',
    '--user-data-dir=' + fs.mkdtempSync(path.join(workDir, 'chrome-')),
    ...extraArgs, 'about:blank',
  ], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'] });
  chrome.stderr.on('data', () => {});
  const out = chrome.stdio[3];
  const inp = chrome.stdio[4];
  let id = 0;
  let buf = '';
  const pending = new Map();
  const listeners = [];
  inp.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\0')) >= 0) {
      const raw = buf.slice(0, i);
      buf = buf.slice(i + 1);
      let m;
      try { m = JSON.parse(raw); } catch { continue; }
      if (m.id && pending.has(m.id)) {
        const p = pending.get(m.id);
        pending.delete(m.id);
        m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result);
      } else if (m.method) {
        listeners.forEach((fn) => fn(m));
      }
    }
  });
  const send = (method, params = {}, sessionId) => {
    const msgId = ++id;
    const msg = { id: msgId, method, params };
    if (sessionId) msg.sessionId = sessionId;
    out.write(JSON.stringify(msg) + '\0');
    return new Promise((resolve, reject) => pending.set(msgId, { resolve, reject }));
  };
  await sleep(300);

  const browser = {
    send,
    close: () => { try { chrome.kill('SIGKILL'); } catch {} },
    // Neuen Tab öffnen (oder den ersten leeren übernehmen) und als Page-Objekt liefern
    async page() {
      let targetId;
      if (!browser._usedFirst) {
        browser._usedFirst = true;
        const { targetInfos } = await send('Target.getTargets');
        targetId = targetInfos.find((t) => t.type === 'page').targetId;
      } else {
        targetId = (await send('Target.createTarget', { url: 'about:blank' })).targetId;
      }
      const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
      await send('Runtime.enable', {}, sessionId);
      await send('Page.enable', {}, sessionId);
      const page = {
        sessionId,
        errors: [],
        console: [],
        async eval(expr) {
          const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true, userGesture: true }, sessionId);
          if (r.exceptionDetails) {
            throw new Error('Seitenfehler: ' + (r.exceptionDetails.exception?.description || r.exceptionDetails.text));
          }
          return r.result.value;
        },
        async goto(url, waitMs = 2500) { await send('Page.navigate', { url }, sessionId); await sleep(waitMs); },
        async reload(waitMs = 2500) { await send('Page.reload', {}, sessionId); await sleep(waitMs); },
        addInitScript: (source) => send('Page.addScriptToEvaluateOnNewDocument', { source }, sessionId),
        async screenshot(file) {
          const r = await send('Page.captureScreenshot', { format: 'png' }, sessionId);
          fs.writeFileSync(file, Buffer.from(r.data, 'base64'));
        },
        setViewport: (width, height) => send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: width < 500 }, sessionId),
      };
      listeners.push((m) => {
        if (m.sessionId !== sessionId) return;
        if (m.method === 'Runtime.exceptionThrown') {
          page.errors.push((m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text).slice(0, 200));
        } else if (m.method === 'Runtime.consoleAPICalled') {
          page.console.push(m.params.args.map((a) => a.value ?? a.description ?? '').join(' ').slice(0, 200));
        }
      });
      return page;
    },
  };
  return browser;
}

// Sammelt alle RTCPeerConnections einer Seite in window.__pcs (vor dem Laden setzen)
const PC_HOOK = `
window.__pcs = [];
(() => { const _PC = window.RTCPeerConnection; if (!_PC) return;
  window.RTCPeerConnection = function (...a) { const pc = new _PC(...a); window.__pcs.push(pc); return pc; };
  window.RTCPeerConnection.prototype = _PC.prototype; })();
window.__rtcStats = async () => {
  const o = { videoFrames: 0, audioBytesPaging: 0, pagingPcs: 0 };
  for (const pc of window.__pcs) {
    if (pc.connectionState === 'closed') continue;
    const kinds = pc.getReceivers().map((r) => r.track && r.track.kind);
    const paging = kinds.length > 0 && kinds.every((k) => k === 'audio');
    if (paging) o.pagingPcs++;
    (await pc.getStats()).forEach((x) => {
      if (x.type !== 'inbound-rtp') return;
      if (x.kind === 'video') o.videoFrames += x.framesDecoded || 0;
      if (x.kind === 'audio' && paging) o.audioBytesPaging += x.bytesReceived || 0;
    });
  }
  return o;
};`;

// Raten über ein Zeitfenster: Video-fps und Audio-Bytes/s der Durchsage-Verbindung
async function rtcRates(page, ms = 2000) {
  const a = await page.eval('window.__rtcStats()');
  await sleep(ms);
  const b = await page.eval('window.__rtcStats()');
  return {
    videoFps: (b.videoFrames - a.videoFrames) / (ms / 1000),
    pagingBytesPerSec: Math.round((b.audioBytesPaging - a.audioBytesPaging) / (ms / 1000)),
    pagingPcs: b.pagingPcs,
  };
}

// Startet in einer (eingeloggten) Seite eine Durchsage mit Fake-Mikrofon
const START_TX = (group, viewers) => `(async () => {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const pc = new RTCPeerConnection({ iceServers: [] });
  stream.getTracks().forEach((t) => pc.addTrack(t, stream));
  const ws = new WebSocket((location.protocol === 'https:' ? 'wss' : 'ws') + '://' + location.host
    + '/ws/voice/tx?group=' + encodeURIComponent(${JSON.stringify(group)}) + '${viewers ? '&viewers=1' : ''}'
    + '&token=' + encodeURIComponent(localStorage.getItem('access_token') || ''));
  const opened = new Promise((r) => (ws.onopen = r));
  window.__tx = { pc, ws, msgs: [] };
  ws.onmessage = async (e) => { const m = JSON.parse(e.data); window.__tx.msgs.push(m);
    if (m.type === 'answer') await pc.setRemoteDescription({ type: 'answer', sdp: m.sdp });
    else if (m.type === 'ice' && m.candidate != null) { try { await pc.addIceCandidate({ candidate: m.candidate, sdpMLineIndex: m.sdpMLineIndex ?? 0 }); } catch {} } };
  pc.onicecandidate = (e) => { if (e.candidate && ws.readyState === 1) ws.send(JSON.stringify({ type: 'ice', candidate: e.candidate.candidate, sdpMLineIndex: e.candidate.sdpMLineIndex })); };
  const offer = await pc.createOffer(); await pc.setLocalDescription(offer); await opened;
  ws.send(JSON.stringify({ type: 'config', viewers: ${viewers ? 'true' : 'false'}, group: ${JSON.stringify(group)} }));
  ws.send(JSON.stringify({ type: 'offer', sdp: offer.sdp }));
  return 'ok'; })()`;
const STOP_TX = `(() => { try { window.__tx.ws.close(); window.__tx.pc.close(); } catch {} return 'ok'; })()`;

// ---------------------------------------------------------------- Server-API
function makeApi(base) {
  const api = {
    base,
    token: null,
    async call(method, p, body) {
      const r = await fetch(base + p, {
        method,
        headers: { ...(api.token ? { Authorization: 'Bearer ' + api.token } : {}), 'Content-Type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const text = await r.text();
      let json = null;
      try { json = text ? JSON.parse(text) : null; } catch {}
      return { status: r.status, json, text };
    },
    get: (p) => api.call('GET', p),
    post: (p, b = {}) => api.call('POST', p, b),
    patch: (p, b) => api.call('PATCH', p, b),
    async login(user = 'admin', pass = process.env.ORS_E2E_PASSWORD || 'e2e-testpass') {
      const r = await api.post('/auth/login', { username: user, password: pass });
      if (!r.json?.access_token) throw new Error('Login fehlgeschlagen: ' + r.text);
      api.token = r.json.access_token;
      return api.token;
    },
    // Browser-Seite einloggen (Token in localStorage)
    async loginPage(page) {
      await page.goto(base + '/ui', 800);
      await page.eval(`localStorage.setItem('access_token', ${JSON.stringify(api.token)}); 'ok'`);
    },
    // Viewer/Pager-Seite gekoppelt öffnen, wie über den QR-Assistenten (Link mit ?pair=)
    async openPaired(page, path, no, waitMs = 2500) {
      const r = await api.post('/api/pairing/start', { viewer_no: no });
      if (!r.json?.code) throw new Error('Kopplungscode fehlt: ' + r.text);
      await page.goto(`${base}${path}/${no}?pair=${encodeURIComponent(r.json.code)}`, waitMs);
    },
    async stopEverything() {
      await api.post('/api/sim/stop_all', {});
      await api.post('/api/admin/settings', { output_rtmp: '' });
    },
  };
  return api;
}

// ---------------------------------------------------------------- Hilfen
// Anzahl Videopakete in den Aufnahmedateien eines Ordners (-1 = nicht lesbar)
function recordingPackets(dir) {
  try {
    const files = fs.readdirSync(dir).filter((f) => f.endsWith('.mkv')).map((f) => path.join(dir, f));
    if (!files.length) return -1;
    const out = execFileSync('ffprobe', ['-v', 'error', '-count_packets', '-select_streams', 'v:0',
      '-show_entries', 'stream=nb_read_packets', '-of', 'csv=p=0', files[0]], { stdio: ['ignore', 'pipe', 'ignore'] });
    return Number(String(out).trim()) || -1;
  } catch { return -1; }
}

function probeStreams(file) {
  try {
    const out = execFileSync('ffprobe', ['-v', 'error', '-count_packets', '-show_entries',
      'stream=codec_type,codec_name,width,height,sample_rate,nb_read_packets', '-of', 'json', file], { stdio: ['ignore', 'pipe', 'ignore'] });
    return JSON.parse(String(out)).streams || [];
  } catch { return []; }
}

function serverRss(pid) {
  try { return Math.round(Number(/VmRSS:\s+(\d+)/.exec(fs.readFileSync(`/proc/${pid}/status`, 'utf8'))[1]) / 1024); } catch { return -1; }
}

async function waitFor(fn, timeoutMs = 15000, stepMs = 500) {
  const t0 = Date.now();
  let last;
  while (Date.now() - t0 < timeoutMs) {
    last = await fn();
    if (last) return last;
    await sleep(stepMs);
  }
  return last;
}

module.exports = {
  sleep, launchBrowser, makeApi, PC_HOOK, rtcRates, START_TX, STOP_TX,
  recordingPackets, probeStreams, serverRss, waitFor,
};
