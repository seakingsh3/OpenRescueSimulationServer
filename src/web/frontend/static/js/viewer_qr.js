(function () {
  const idEl = document.getElementById('qrViewerId');
  const linkEl = document.getElementById('qrLink');
  const hintEl = document.getElementById('qrHint');
  const copyBtn = document.getElementById('qrCopyBtn');
  const openBtn = document.getElementById('qrOpenBtn');
  const statusEl = document.getElementById('qrStatus');
  const qrImg = document.getElementById('qrImg');
  const generateBtn = document.getElementById('qrGenerateBtn');
  const resetBtn = document.getElementById('qrResetBtn');
  const stepWebrtc = document.getElementById('stepWebrtc');
  const stepConfirm = document.getElementById('stepConfirm');
  const stepWebrtcOk = document.getElementById('stepWebrtcOk');
  const stepsEl = document.getElementById('qrSteps');
  const wizardModal = document.getElementById('qrWizardModal');
  const wizardOpen = document.getElementById('qrOpenWizard');
  const wizardClose = document.getElementById('qrWizardClose');

  if (!idEl || !linkEl || !qrImg) return;

  let serverHost = '';

  const storageId = localStorage.getItem('viewer_qr_id');
  if (storageId) idEl.value = storageId;
  // Altlast aus dem entfernten MistServer-Modus
  localStorage.removeItem('viewer_qr_mode');
  localStorage.removeItem('viewer_qr_mist_type');

  function updateQrImage(text) {
    qrImg.src = `/api/qr?text=${encodeURIComponent(text)}`;
  }

  function clearOutput() {
    linkEl.value = '';
    if (openBtn) openBtn.removeAttribute('href');
    if (statusEl) statusEl.textContent = 'Warte auf Eingaben...';
    if (hintEl) hintEl.textContent = '';
    qrImg.removeAttribute('src');
  }

  function isLoopback(host) {
    return !host || host === 'localhost' || host === '127.0.0.1' || host === '::1';
  }

  function buildLink() {
    const viewerId = (idEl.value || '1').trim() || '1';
    const protocol = location.protocol === 'https:' ? 'https:' : 'http:';
    const port = location.port ? `:${location.port}` : '';
    return `${protocol}//${serverHost}${port}/viewer/${encodeURIComponent(viewerId)}`;
  }

  // Einmal-Kopplungscode für diese Viewer-Nummer holen: Wer den QR-Code scannt, ist damit
  // gekoppelt (gilt 2 Minuten, nur für ein Gerät). Ohne Code müsste am Gerät eine PIN rein.
  async function pairingCode(viewerId) {
    const no = Number(viewerId);
    if (!Number.isInteger(no) || no <= 0) return null;
    try {
      const t = localStorage.getItem('access_token') || '';
      const r = await fetch('/api/pairing/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...(t ? { Authorization: `Bearer ${t}` } : {}) },
        body: JSON.stringify({ viewer_no: no }),
      });
      return r.ok ? (await r.json()).code || null : null;
    } catch { return null; }
  }

  async function updateUi() {
    const viewerId = (idEl.value || '1').trim() || '1';
    const code = await pairingCode(viewerId);
    const link = buildLink() + (code ? `?pair=${encodeURIComponent(code)}` : '');
    linkEl.value = link;
    if (openBtn) openBtn.href = link;
    if (hintEl) {
      hintEl.textContent = isLoopback(serverHost)
        ? 'Host/IP konnte nicht automatisch ermittelt werden.'
        : code
          ? 'Koppelt das Gerät beim Öffnen – gilt 2 Minuten und nur für ein Gerät.'
          : 'Ohne Kopplungscode: am Gerät die PIN aus „Gerät koppeln“ eingeben.';
    }
    if (statusEl) statusEl.textContent = `Viewer-ID ${viewerId}`;
    updateQrImage(link);
  }

  function setStepper(idx) {
    if (!stepsEl) return;
    stepsEl.querySelectorAll('.step').forEach((el, i) => {
      el.classList.toggle('step-primary', i <= idx);
    });
  }

  function showStep(step) {
    if (stepWebrtc) stepWebrtc.classList.toggle('hidden', step !== 'webrtc');
    if (stepConfirm) stepConfirm.classList.toggle('hidden', step !== 'confirm');
    setStepper(step === 'confirm' ? 1 : 0);
  }

  if (stepWebrtcOk) {
    stepWebrtcOk.addEventListener('click', () => {
      localStorage.setItem('viewer_qr_id', idEl.value.trim() || '1');
      showStep('confirm');
      clearOutput();
    });
  }

  if (copyBtn) {
    copyBtn.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(linkEl.value);
        if (statusEl) statusEl.textContent = 'Link kopiert';
        closeWizard();
      } catch {
        if (statusEl) statusEl.textContent = 'Kopieren fehlgeschlagen';
      }
    });
  }

  if (generateBtn) {
    generateBtn.addEventListener('click', () => {
      if (isLoopback(serverHost)) {
        if (statusEl) statusEl.textContent = 'Host/IP konnte nicht ermittelt werden.';
        return;
      }
      updateUi().then(closeWizard);
    });
  }

  if (resetBtn) {
    resetBtn.addEventListener('click', () => {
      idEl.value = '1';
      localStorage.removeItem('viewer_qr_id');
      showStep('webrtc');
      clearOutput();
    });
  }

  function openWizard() {
    if (wizardModal) wizardModal.classList.remove('hidden');
    document.body.classList.add('modal-open');
    showStep('webrtc');
  }

  function closeWizard() {
    if (wizardModal) wizardModal.classList.add('hidden');
    document.body.classList.remove('modal-open');
  }

  if (wizardOpen) wizardOpen.addEventListener('click', (e) => { e.preventDefault(); openWizard(); });
  if (wizardClose) wizardClose.addEventListener('click', (e) => { e.preventDefault(); closeWizard(); });
  if (wizardModal) wizardModal.addEventListener('click', (e) => { if (e.target === wizardModal) closeWizard(); });
  window.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeWizard(); });

  function setHostFromServer(ip) {
    const val = (ip || '').trim();
    serverHost = isLoopback(val) ? '' : val;
  }

  fetch('/api/host/ip')
    .then((r) => (r.ok ? r.json() : null))
    .then((res) => {
      const ip = (res?.ip || '').trim();
      if (ip) setHostFromServer(ip);
    })
    .catch(() => {})
    .finally(() => {
      if (!serverHost) {
        const fallback = location.hostname || '';
        if (!isLoopback(fallback)) setHostFromServer(fallback);
      }
    });

  showStep('webrtc');
  clearOutput();
})();
