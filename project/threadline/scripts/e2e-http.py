"""Drive real traffic through a forward: point it at 1.1.1.1:80 and speak HTTP.

Usage: python scripts/e2e-http.py <server_id> <local_port>
"""

import json
import os
import socket
import sys
import time
import urllib.error
import urllib.request

CONTROL = (
    os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
    if "PUBLIC" in os.environ
    else "/mnt/c/Users/Public/Threadline/control.json"
)

with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
headers = {"Authorization": f"Bearer {control['token']}"}
server_id, local_port = sys.argv[1], int(sys.argv[2])


def call(method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    sent = dict(headers)
    if data is not None:
        sent["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=data, method=method, headers=sent)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            raw = resp.read().decode()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode(errors="replace")


def state_of(tunnel_id):
    _, listing = call("GET", "/tunnels")
    for item in listing["items"]:
        if item["definition"]["id"] == tunnel_id:
            return item["status"]
    return None


_, listing = call("GET", "/tunnels")
tunnel_id = None
for item in listing["items"]:
    if item["definition"]["name"] == "e2e":
        tunnel_id = item["definition"]["id"]

if not tunnel_id:
    status, created = call("POST", "/tunnels", {
        "name": "e2e",
        "server_id": server_id,
        "local_port": local_port,
        "remote_port": 22,
    })
    tunnel_id = created["definition"]["id"]

# Point the forward at something reachable from the server.
call("PATCH", f"/tunnels/{tunnel_id}", {
    "remote_host": "1.1.1.1",
    "remote_port": 80,
})
call("POST", f"/tunnels/{tunnel_id}/stop")
time.sleep(0.5)
status, payload = call("POST", f"/tunnels/{tunnel_id}/start")
print("start:", status, payload if status >= 400 else payload["status"]["state"])
for _ in range(10):
    time.sleep(0.5)
    current = state_of(tunnel_id)
    if current["state"] == "running":
        break
print("state:", current["state"], current["last_error"] or "")

try:
    sock = socket.create_connection(("127.0.0.1", local_port), timeout=5)
    sock.settimeout(8)
    sock.sendall(b"GET / HTTP/1.0\r\nHost: 1.1.1.1\r\n\r\n")
    data = sock.recv(2048)
    head = data.decode("utf-8", "replace").split("\r\n")[0]
    print(f"through the forward: {len(data)} bytes, first line = {head!r}")
    sock.close()
except OSError as error:
    print("through the forward failed:", error)
