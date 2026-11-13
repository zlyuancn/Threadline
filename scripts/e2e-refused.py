"""Point a forward at a closed port on the server and check the error surfaces.

Usage: python scripts/e2e-refused.py <server_id> <local_port>
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


def status_of(tunnel_id):
    _, listing = call("GET", "/tunnels")
    for item in listing["items"]:
        if item["definition"]["id"] == tunnel_id:
            return item["status"]
    return None


_, listing = call("GET", "/tunnels")
for item in listing["items"]:
    if item["definition"]["name"] == "refused":
        call("DELETE", f"/tunnels/{item['definition']['id']}")

_, created = call("POST", "/tunnels", {
    "name": "refused",
    "server_id": server_id,
    "local_port": local_port,
    "remote_port": 1,
})
tunnel_id = created["definition"]["id"]

call("POST", f"/tunnels/{tunnel_id}/start")
time.sleep(1.5)
print("state:", status_of(tunnel_id)["state"])

try:
    sock = socket.create_connection(("127.0.0.1", local_port), timeout=5)
    sock.settimeout(12)
    data = sock.recv(64)
    print("received:", data)
except OSError as error:
    print("local error:", error)

time.sleep(1.0)
final = status_of(tunnel_id)
print("state after:", final["state"])
print("last_error :", final["last_error"])
