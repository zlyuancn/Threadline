"""检查本地端口冲突时的报文（要点名占用的转发，别再让界面静默成功）。"""

import json
import os
import urllib.error
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
headers = {
    "Authorization": f"Bearer {control['token']}",
    "Content-Type": "application/json",
}

items = json.load(
    urllib.request.urlopen(urllib.request.Request(base + "/tunnels", headers=headers))
)["items"]
if not items:
    raise SystemExit("no forward to clash with")

occupied = items[0]["definition"]
body = json.dumps(
    {
        "name": "conflict-probe",
        "server_id": occupied["server_id"],
        "local_host": "127.0.0.1",
        "local_port": occupied["local_port"],
        "remote_host": "127.0.0.1",
        "remote_port": 1,
        "auto_start": False,
        "scheme": "tcp",
    }
).encode()

req = urllib.request.Request(f"{base}/tunnels", data=body, method="POST", headers=headers)
try:
    with urllib.request.urlopen(req, timeout=10) as resp:
        print("unexpected success:", resp.status)
except urllib.error.HTTPError as err:
    payload = json.loads(err.read().decode())
    print("status:", err.code)
    print("message:", payload["error"]["message"])
