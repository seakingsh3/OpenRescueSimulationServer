import { authFetch, withAuthHeaders } from '/static/js/http.js';

// API läuft immer auf demselben Ursprung wie die Oberfläche (HTTP 8393, HTTPS 8443
// oder ein per ORS_BIND gesetzter Port). Ein fester Port würde bei HTTPS/anderen Ports
// ins Leere laufen (Login-Prüfung scheitert, Admin-Funktionen bleiben gesperrt).
export const API_BASE_URL = typeof window !== 'undefined' ? window.location.origin : '';

async function parseJsonOrThrow(res, errorMsg) {
  if (!res.ok) {
    let payload = null;
    try { payload = await res.json(); } catch {}
    const err = new Error(errorMsg || `HTTP ${res.status}`);
    err.status = res.status;
    err.payload = payload;
    throw err;
  }
  return res.json();
}

const api = {
  async login(username, password) {
    const res = await fetch(`${API_BASE_URL}/auth/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
    });
    const data = await parseJsonOrThrow(res, 'Login fehlgeschlagen');
    localStorage.setItem('token', data.access_token);
    localStorage.setItem('access_token', data.access_token);
    if (data.username) localStorage.setItem('username', data.username);
    return data;
  },

  async me() {
    const res = await authFetch(`${API_BASE_URL}/auth/me`, {
      headers: withAuthHeaders(),
    });
    return parseJsonOrThrow(res, 'Nicht authentifiziert');
  },

  async getSimulationStatus() {
    const res = await authFetch(`${API_BASE_URL}/api/sim/status`, {
      headers: withAuthHeaders(),
    });
    return parseJsonOrThrow(res, 'Fehler beim Abrufen des Simulation-Status');
  },

  async startSimulation(name, viewerIds) {
    const body = {};
    if (name) body.name = name;
    const vids = Array.isArray(viewerIds)
      ? viewerIds.filter((v) => v != null && !isNaN(v) && Number(v) > 0).map(Number)
      : [];
    if (vids.length) body.viewer_ids = vids;
    const res = await authFetch(`${API_BASE_URL}/api/sim/start`, {
      method: 'POST',
      headers: withAuthHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify(body),
    });
    return parseJsonOrThrow(res, 'sim_start failed');
  },

  async stopSimulation() {
    const res = await authFetch(`${API_BASE_URL}/api/sim/stop`, {
      method: 'POST',
      headers: withAuthHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify({}),
    });
    return parseJsonOrThrow(res, 'Fehler beim Stoppen der Simulation');
  },

  async getViewers() {
    const res = await authFetch(`${API_BASE_URL}/api/viewers`, {
      headers: withAuthHeaders(),
    });
    return parseJsonOrThrow(res, 'Fehler beim Abrufen der Viewer-Liste');
  },

  // Compatibility: existing code expects array directly
  async getMarkerTypes() {
    const res = await authFetch(`${API_BASE_URL}/api/marker_types`, {
      headers: withAuthHeaders(),
    });
    const data = await parseJsonOrThrow(res, 'Fehler beim Laden der Marker-Typen');
    return Array.isArray(data?.marker_types) ? data.marker_types : [];
  },

  // Compatibility: old caller sends extra fields; backend ignores unknown fields.
  async addMarker(payload) {
    const body = {
      marker_type_id: payload?.marker_type_id,
      text: payload?.text || '',
    };
    const res = await authFetch(`${API_BASE_URL}/api/markers`, {
      method: 'POST',
      headers: withAuthHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify(body),
    });
    return parseJsonOrThrow(res, 'Fehler beim Setzen des Markers');
  },

  // Compatibility: existing code expects array directly
  async getMarkers(sessionId) {
    const sid = encodeURIComponent(sessionId);
    const res = await authFetch(`${API_BASE_URL}/api/sessions/${sid}/markers`, {
      headers: withAuthHeaders(),
    });
    const data = await parseJsonOrThrow(res, 'Fehler beim Laden der Marker');
    return Array.isArray(data?.markers) ? data.markers : [];
  },
};

export default api;
