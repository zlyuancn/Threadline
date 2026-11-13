"""Print the live state of the core: host keys per tunnel and each forward's status.

Usage: python scripts/state.py
"""

import json
import os
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


def get(path):
    req = urllib.request.Request(base + path, headers=headers)
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.loads(resp.read().decode())


for server in get("/servers")["items"]:
    view = get(f"/servers/{server['id']}/hostkey")
    print(
        f"server  {server['name']:<14} trusted={view['trusted_fingerprint']} "
        f"pending={view['pending_fingerprint']}"
    )

for item in get("/tunnels")["items"]:
    status = item["status"]
    print(
        f"forward {item['definition']['name']:<14} state={status['state']:<12} "
        f"enabled={item['definition']['auto_start']} err={status['last_error']}"
    )
