"""验证状态变更事件流：改一条转发后，SSE 应立刻推一条 changed。"""

import json
import os
import socket
import threading
import time
import urllib.error
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
auth = {"Authorization": f"Bearer {control['token']}"}


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        base + path,
        data=data,
        method=method,
        headers={**auth, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            raw = resp.read().decode()
            return json.loads(raw) if raw.strip() else None
    except urllib.error.HTTPError as exc:
        print("  ->", method, path, "HTTP", exc.code, exc.read(400).decode(errors="replace"))
        raise


items = call("GET", "/tunnels")["items"]
if not items:
    raise SystemExit("没有转发可测")
target = items[0]["definition"]

try:
    stream = urllib.request.urlopen(
        urllib.request.Request(base + "/events", headers={**auth, "Accept": "text/event-stream"}),
        timeout=8,
    )
except urllib.error.HTTPError as exc:
    print("HTTP", exc.code, exc.read().decode(errors="replace"))
    raise SystemExit(1)
stream.fp.raw._sock.settimeout(6)  # noqa: SLF001 - 需要读超时，否则会永久阻塞

triggered_at = {}


def trigger():
    time.sleep(0.5)
    triggered_at["t"] = time.perf_counter()
    call("PATCH", f"/tunnels/{target['id']}", {"auto_start": not target["auto_start"]})


threading.Thread(target=trigger, daemon=True).start()

received_at = None
deadline = time.time() + 6
while time.time() < deadline:
    try:
        line = stream.readline().decode()
    except socket.timeout:
        break
    if line.startswith("event:") and "changed" in line:
        received_at = time.perf_counter()
        break

if received_at and "t" in triggered_at:
    print(f"事件到达延迟 {(received_at - triggered_at['t']) * 1000:.0f} ms")
    print("SSE: GREEN")
else:
    print("没收到事件")
    print("SSE: FAIL")

# 还原
call("PATCH", f"/tunnels/{target['id']}", {"auto_start": target["auto_start"]})
