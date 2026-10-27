let _lastAuthNoticeTs = 0;

export function getAuthToken() {
  return localStorage.getItem('access_token') || localStorage.getItem('token') || '';
}

export function withAuthHeaders(headers = {}) {
  const token = getAuthToken();
  return token ? { ...headers, Authorization: `Bearer ${token}` } : headers;
}

function notifyAuthRequired() {
  const now = Date.now();
  if (now - _lastAuthNoticeTs < 2500) return;
  _lastAuthNoticeTs = now;
  window.dispatchEvent(new CustomEvent('ors:auth-required'));
}

export async function authFetch(url, init = {}) {
  const next = { ...init, headers: withAuthHeaders(init.headers || {}) };
  const res = await fetch(url, next);
  if (res.status === 401) notifyAuthRequired();
  return res;
}

export async function authJsonFetch(url, init = {}) {
  const res = await authFetch(url, init);
  const ct = res.headers.get('content-type') || '';
  const parse = async () => (ct.includes('application/json') ? res.json() : res.text());
  if (!res.ok) {
    const payload = await parse().catch(() => null);
    const err = new Error(`${init.method || 'GET'} ${url} -> ${res.status}`);
    err.status = res.status;
    err.payload = payload;
    throw err;
  }
  return await parse();
}
