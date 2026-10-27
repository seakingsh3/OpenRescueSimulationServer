#!/usr/bin/env node
// Runner der Ende-zu-Ende-Tests. Aufruf über tests/e2e/run.sh (baut vorher), oder direkt:
//   node tests/e2e/run.js [--slow] [--keep] [filter …]
// Startet einen eigenen Server mit frischer Datenbank in einem Temp-Ordner, schließt die
// Ersteinrichtung über die API ab und führt alle tests/e2e/tests/*.test.js nacheinander aus.
'use strict';
const { spawn, execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const lib = require('./lib');

const ROOT = path.resolve(__dirname, '..', '..');
const args = process.argv.slice(2);
const SLOW = args.includes('--slow');
const KEEP = args.includes('--keep');
const filters = args.filter((a) => !a.startsWith('--'));
const PORT = Number(process.env.ORS_E2E_PORT || 18493);
const BASE = `http://127.0.0.1:${PORT}`;
const PASSWORD = process.env.ORS_E2E_PASSWORD || 'e2e-testpass';
const BIN = process.env.ORS_E2E_BIN || path.join(ROOT, 'target', 'debug', 'openresquesim_server');

const workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'ors-e2e-'));
const dataDir = path.join(workDir, 'data');
fs.mkdirSync(dataDir);
const logFile = path.join(workDir, 'server.log');

let server = null;

function have(cmd) {
  try { execFileSync('sh', ['-c', `command -v ${cmd}`], { stdio: 'ignore' }); return true; } catch { return false; }
}

async function startServer(extraEnv = {}) {
  const logFd = fs.openSync(logFile, 'a');
  server = spawn(BIN, [], {
    cwd: ROOT, // Server findet src/web/... relativ zum Projektordner
    env: { ...process.env, ORS_DATA_DIR: dataDir, ORS_BIND: `127.0.0.1:${PORT}`, ORS_DEFAULT_ADMIN_PASSWORD: PASSWORD, ...extraEnv },
    stdio: ['ignore', logFd, logFd],
  });
  const ok = await lib.waitFor(async () => {
    try { return (await fetch(BASE + '/api/sim/status')).ok; } catch { return false; }
  }, 20000, 300);
  if (!ok) throw new Error('Server startet nicht, siehe ' + logFile);
}

async function stopServer() {
  if (!server) return;
  const s = server;
  server = null;
  s.kill('SIGTERM');
  const exited = await Promise.race([new Promise((r) => s.once('exit', () => r(true))), lib.sleep(5000).then(() => false)]);
  if (!exited) s.kill('SIGKILL');
}

async function setup() {
  await startServer();
  const api = lib.makeApi(BASE);
  const r = await api.post('/api/setup/complete', {
    admin_username: 'admin', admin_password: PASSWORD, auth_secret: 'e2e-' + Date.now(),
    settings: {
      record_enabled: true, record_format: 'mkv', record_segment_secs: 300, record_max_files: 0,
      record_prefix: 'rec', record_dir: path.join(dataDir, 'recordings'), audio_device: '',
    },
  });
  if (r.status !== 200) throw new Error('Setup fehlgeschlagen: ' + r.status + ' ' + r.text);
  await stopServer();
  await startServer();
}

async function main() {
  if (!fs.existsSync(BIN)) throw new Error('Server-Binary fehlt: ' + BIN + ' (zuerst cargo build)');
  for (const cmd of ['google-chrome', 'ffprobe']) {
    if (!have(process.env.CHROME_BIN && cmd === 'google-chrome' ? process.env.CHROME_BIN : cmd)) throw new Error(`Benötigt: ${cmd}`);
  }
  const files = fs.readdirSync(path.join(__dirname, 'tests')).filter((f) => f.endsWith('.test.js')).sort()
    .filter((f) => !filters.length || filters.some((x) => f.includes(x)));
  console.log(`E2E: ${files.length} Testdatei(en), Arbeitsordner ${workDir}`);
  await setup();

  const results = [];
  for (const file of files) {
    const mod = require(path.join(__dirname, 'tests', file));
    if (mod.slow && !SLOW) { results.push({ file, name: mod.name, skipped: 'langsam (--slow)' }); continue; }
    const missing = (mod.needs || []).filter((c) => !have(c));
    if (missing.length) { results.push({ file, name: mod.name, skipped: 'fehlt: ' + missing.join(', ') }); continue; }

    const checks = [];
    const browsers = [];
    const t = {
      base: BASE, api: lib.makeApi(BASE), workDir, dataDir, lib,
      get serverPid() { return server && server.pid; },
      serverLog: () => fs.readFileSync(logFile, 'utf8').replace(/\x1b\[[0-9;]*m/g, ''),
      logMark: () => fs.statSync(logFile).size,
      logSince: (mark) => fs.readFileSync(logFile).subarray(mark).toString('utf8').replace(/\x1b\[[0-9;]*m/g, ''),
      async browser() { const b = await lib.launchBrowser(workDir); browsers.push(b); return b; },
      check(label, ok, detail) {
        checks.push({ label, ok: !!ok, detail });
        console.log(`   ${ok ? '✔' : '✘'} ${label}${detail !== undefined ? '  — ' + (typeof detail === 'string' ? detail : JSON.stringify(detail)) : ''}`);
      },
      restartServer: async (env) => { await stopServer(); await startServer(env); },
    };
    console.log(`\n▶ ${mod.name}  (${file})`);
    const t0 = Date.now();
    try {
      await t.api.login();
      await mod.run(t);
    } catch (e) {
      checks.push({ label: 'Testlauf ohne Ausnahme', ok: false, detail: e.message });
      console.log(`   ✘ Ausnahme: ${e.stack || e.message}`);
    } finally {
      browsers.forEach((b) => b.close());
      try { await t.api.login(); await t.api.stopEverything(); } catch {}
      await lib.sleep(1000);
    }
    results.push({ file, name: mod.name, checks, secs: Math.round((Date.now() - t0) / 1000) });
  }

  await stopServer();
  console.log('\n================ Ergebnis ================');
  let failed = 0;
  for (const r of results) {
    if (r.skipped) { console.log(`  ○ ${r.name}: übersprungen (${r.skipped})`); continue; }
    const bad = r.checks.filter((c) => !c.ok);
    failed += bad.length;
    console.log(`  ${bad.length ? '✘' : '✔'} ${r.name}: ${r.checks.length - bad.length}/${r.checks.length} (${r.secs} s)`);
    bad.forEach((c) => console.log(`      ✘ ${c.label}${c.detail !== undefined ? ' — ' + JSON.stringify(c.detail) : ''}`));
  }
  console.log(failed ? `\n${failed} Prüfung(en) fehlgeschlagen. Serverlog: ${logFile}` : '\nAlle Prüfungen bestanden.');
  if (!KEEP && !failed) fs.rmSync(workDir, { recursive: true, force: true });
  else console.log('Arbeitsordner bleibt erhalten: ' + workDir);
  process.exit(failed ? 1 : 0);
}

process.on('SIGINT', async () => { await stopServer(); process.exit(130); });
main().catch(async (e) => { console.error('E2E abgebrochen:', e.message); await stopServer(); console.error('Serverlog: ' + logFile); process.exit(2); });
