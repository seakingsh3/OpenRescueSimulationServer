// Kopplung von Viewer-Bildschirmen und Pagern (siehe src/pairing.rs).
// Eingebunden von /viewer/<n> und /pager/<n>. Stellt window.ORSPairing bereit:
//   ORSPairing.key(n)          gespeicherter Geräteschlüssel für Nummer n (oder '')
//   ORSPairing.fromLink(n)     koppelt automatisch, wenn die Adresse ?pair=<code> enthält
//   ORSPairing.prompt(n)       zeigt „Bitte koppeln“ mit PIN-Feld; Promise → Schlüssel
(function () {
  'use strict';
  const storeKey = (n) => `ors_device_key_${n}`;

  function key(n) {
    try { return localStorage.getItem(storeKey(n)) || ''; } catch { return ''; }
  }

  function save(n, k) {
    try { localStorage.setItem(storeKey(n), k); } catch {}
  }

  function forget(n) {
    try { localStorage.removeItem(storeKey(n)); } catch {}
  }

  async function claim(n, code) {
    const r = await fetch('/api/pairing/claim', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ code: String(code).trim(), viewer_no: Number(n), label: navigator.userAgent.slice(0, 80) }),
    });
    if (!r.ok) throw new Error(r.status === 403 ? 'Code ungültig oder abgelaufen' : 'Kopplung fehlgeschlagen (' + r.status + ')');
    const j = await r.json();
    save(n, j.device_key);
    return j.device_key;
  }

  // QR-Link: /viewer/<n>?pair=<code> → koppeln und den Code aus der Adresse entfernen
  async function fromLink(n) {
    const url = new URL(window.location.href);
    const code = url.searchParams.get('pair');
    if (!code) return null;
    url.searchParams.delete('pair');
    window.history.replaceState(null, '', url.pathname + (url.search || '') + url.hash);
    try {
      return await claim(n, code);
    } catch (e) {
      console.warn('[pairing] QR-Kopplung fehlgeschlagen:', e.message);
      return null;
    }
  }

  let open = null;
  function prompt(n, message) {
    if (open) return open;
    forget(n);
    open = new Promise((resolve) => {
      const wrap = document.createElement('div');
      wrap.id = 'orsPairOverlay';
      wrap.setAttribute('role', 'dialog');
      wrap.style.cssText = 'position:fixed;inset:0;z-index:2147483000;display:flex;align-items:center;justify-content:center;'
        + 'background:rgba(5,10,20,.92);color:#e5e7eb;font-family:system-ui,sans-serif;padding:16px;';
      wrap.innerHTML = `
        <div style="max-width:420px;width:100%;text-align:center;background:#111827;border:1px solid #334155;border-radius:16px;padding:28px 22px;box-shadow:0 20px 60px rgba(0,0,0,.5)">
          <div style="font-size:40px;line-height:1">🔗</div>
          <h1 style="font-size:22px;margin:10px 0 4px">Bitte koppeln</h1>
          <p style="margin:0 0 16px;color:#94a3b8;font-size:14px">Bildschirm Nr. ${String(n).replace(/[^0-9]/g, '')} ist noch nicht freigegeben.
            In der Oberfläche unter <b>Einstellungen → Kameras &amp; Räume → Aktive Viewer</b> auf
            <b>„Gerät koppeln“</b> tippen und die angezeigte PIN hier eingeben.</p>
          <input id="orsPairPin" inputmode="numeric" autocomplete="one-time-code" maxlength="6" placeholder="••••••"
            style="width:100%;box-sizing:border-box;font-size:34px;letter-spacing:.4em;text-align:center;padding:10px;border-radius:10px;border:1px solid #475569;background:#0b1220;color:#fff">
          <button id="orsPairOk" style="margin-top:14px;width:100%;padding:12px;font-size:17px;border-radius:10px;border:0;background:#14b8a6;color:#04201c;font-weight:700;cursor:pointer">Koppeln</button>
          <p id="orsPairMsg" style="min-height:20px;margin:12px 0 0;font-size:14px;color:#fca5a5">${message || ''}</p>
        </div>`;
      document.body.appendChild(wrap);
      const pin = wrap.querySelector('#orsPairPin');
      const ok = wrap.querySelector('#orsPairOk');
      const msg = wrap.querySelector('#orsPairMsg');
      pin.addEventListener('input', () => { pin.value = pin.value.replace(/\D/g, '').slice(0, 6); });
      const submit = async () => {
        if (pin.value.length !== 6) { msg.textContent = 'Bitte die 6-stellige PIN eingeben.'; return; }
        ok.disabled = true;
        msg.style.color = '#94a3b8';
        msg.textContent = 'Kopple …';
        try {
          const k = await claim(n, pin.value);
          wrap.remove();
          open = null;
          resolve(k);
        } catch (e) {
          msg.style.color = '#fca5a5';
          msg.textContent = e.message;
          pin.value = '';
          ok.disabled = false;
          pin.focus();
        }
      };
      ok.addEventListener('click', submit);
      pin.addEventListener('keydown', (e) => { if (e.key === 'Enter') submit(); });
      setTimeout(() => pin.focus(), 50);
    });
    return open;
  }

  window.ORSPairing = { key, fromLink, prompt, forget };
})();
