"""Simulierter Raumlautsprecher (nur Standardbibliothek).

Meldet sich per WebSocket unter /ws/voice/speaker an, schickt Heartbeats, zählt
eingehende RTP-Pakete auf seinem UDP-Port und schreibt seinen Zustand regelmäßig
als JSON in <statusdatei>. Empfangene Befehle (test_tone, reboot, …) landen dort auch.

Aufruf: fake_speaker.py <host:port> <device_id> <name> <udp_port> <statusdatei> [schluesseldatei]

Mit <schluesseldatei> verhält er sich wie die neue Firmware: meldet `key_support`, schickt
den gespeicherten Geräteschlüssel mit und speichert einen per `pair` erhaltenen Schlüssel.
Ohne sie wie alte Firmware (keine Freigabe nötig, „alt · ungesichert“).
"""
import base64, json, os, socket, struct, sys, threading, time

host_port, device_id, name, udp_port, status_file = sys.argv[1:6]
key_file = sys.argv[6] if len(sys.argv) > 6 else None
host, port = host_port.split(':')
udp_port = int(udp_port)

state = {'device_id': device_id, 'rtp_packets': 0, 'last_pt': None, 'commands': [], 'registered': None, 'auth': None, 'error': None}
last_rtp = [0.0]
lock = threading.Lock()


def udp_loop():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(('127.0.0.1', udp_port))
    while True:
        data, _ = s.recvfrom(4096)
        with lock:
            state['rtp_packets'] += 1
            if len(data) > 1:
                state['last_pt'] = data[1] & 0x7F
            last_rtp[0] = time.time()


def ws_connect():
    sock = socket.create_connection((host, int(port)))
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall((f'GET /ws/voice/speaker HTTP/1.1\r\nHost: {host_port}\r\nUpgrade: websocket\r\n'
                  f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
    resp = b''
    while b'\r\n\r\n' not in resp:
        resp += sock.recv(1)
    assert b' 101 ' in resp.split(b'\r\n')[0], resp
    return sock


def ws_send(sock, text):
    payload = text.encode()
    mask = os.urandom(4)
    hdr = bytes([0x81])
    n = len(payload)
    if n < 126:
        hdr += bytes([0x80 | n])
    elif n < 65536:
        hdr += bytes([0x80 | 126]) + struct.pack('>H', n)
    else:
        hdr += bytes([0x80 | 127]) + struct.pack('>Q', n)
    sock.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))


def recv_exact(sock, n):
    buf = b''
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError('closed')
        buf += chunk
    return buf


def ws_recv(sock):
    b1, b2 = recv_exact(sock, 2)
    n = b2 & 0x7F
    if n == 126:
        n = struct.unpack('>H', recv_exact(sock, 2))[0]
    elif n == 127:
        n = struct.unpack('>Q', recv_exact(sock, 8))[0]
    data = recv_exact(sock, n)
    return b1 & 0x0F, data


def write_status():
    while True:
        with lock:
            snap = dict(state, playing=time.time() - last_rtp[0] < 1.0)
        tmp = status_file + '.tmp'
        with open(tmp, 'w') as f:
            json.dump(snap, f)
        os.replace(tmp, status_file)
        time.sleep(0.5)


threading.Thread(target=udp_loop, daemon=True).start()
threading.Thread(target=write_status, daemon=True).start()

sock = ws_connect()
reg = {'type': 'register', 'device_id': device_id, 'name': name, 'group': 'default',
       'rtp_port': udp_port, 'codec': 'opus', 'version': 'fake-1', 'ip': '127.0.0.1'}
if key_file:
    reg['key_support'] = True
    if os.path.exists(key_file):
        reg['key'] = open(key_file).read().strip()
ws_send(sock, json.dumps(reg))


def heartbeat():
    while True:
        with lock:
            playing = time.time() - last_rtp[0] < 1.0
        try:
            ws_send(sock, json.dumps({'type': 'heartbeat', 'playing': playing, 'rms_db': -30.0 if playing else -120.0}))
        except OSError:
            return
        time.sleep(2)


threading.Thread(target=heartbeat, daemon=True).start()
while True:
    op, data = ws_recv(sock)
    if op == 8:
        break
    if op != 1:
        continue
    msg = json.loads(data)
    with lock:
        if msg.get('type') == 'registered':
            state['registered'] = msg.get('name')
            state['auth'] = msg.get('auth')
        elif msg.get('type') == 'pair' and key_file:
            with open(key_file, 'w') as f:
                f.write(msg.get('key', ''))
            state['commands'].append('pair')
        elif msg.get('type') == 'unpair' and key_file:
            if os.path.exists(key_file):
                os.remove(key_file)
            state['commands'].append('unpair')
        elif msg.get('type') == 'error':
            state['error'] = msg.get('message')
        elif msg.get('type') not in ('stream',):
            state['commands'].append(msg.get('type'))
        else:
            state['commands'].append('stream:' + str(msg.get('active')))
