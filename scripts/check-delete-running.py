"""验证：运行中的端口转发能不能删（应当先停、再移除，端口随之释放）。"""

import json
import os
import time
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
headers = {
    "Authorization": f"Bearer {control['token']}",
    "Content-Type": "application/json",
}
NAME = "delete-running-probe"


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(req, timeout=20) as resp:
        raw = resp.read().decode()
        return resp.status, (json.loads(raw) if raw.strip() else None)


def find(name):
    _, body = call("GET", "/tunnels")
    for item in body["items"]:
        if item["definition"]["name"] == name:
            return item
    return None


server_id = call("GET", "/servers")[1]["items"][0]["id"]

existing = find(NAME)
if existing:
    call("DELETE", f"/tunnels/{existing['definition']['id']}")

status, created = call(
    "POST",
    "/tunnels",
    {
        "name": NAME,
        "server_id": server_id,
        "local_host": "127.0.0.1",
        "local_port": 18093,
        "remote_host": "127.0.0.1",
        "remote_port": 16100,
        "auto_start": True,
        "scheme": "tcp",
    },
)
tunnel_id = created["definition"]["id"]

deadline = time.time() + 10
state = None
while time.time() < deadline:
    item = find(NAME)
    state = item["status"]["state"] if item else None
    if state == "running":
        break
    time.sleep(0.4)

print("state before delete:", state)
status, _ = call("DELETE", f"/tunnels/{tunnel_id}")
print("delete status:", status)
print("still listed:", find(NAME) is not None)

# 端口要跟着释放：再用同一个本地端口建一条，应当成功
status, again = call(
    "POST",
    "/tunnels",
    {
        "name": NAME + "-again",
        "server_id": server_id,
        "local_host": "127.0.0.1",
        "local_port": 18093,
        "remote_host": "127.0.0.1",
        "remote_port": 16100,
        "auto_start": False,
        "scheme": "tcp",
    },
)
print("reuse same port:", status)
call("DELETE", f"/tunnels/{again['definition']['id']}")
