"""验证改一条转发的开关不会影响其它转发（id 撞车时会串）。"""

import json
import os
import sys
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
headers = {
    "Authorization": f"Bearer {control['token']}",
    "Content-Type": "application/json",
}


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(req, timeout=15) as resp:
        raw = resp.read().decode()
        return json.loads(raw) if raw.strip() else None


items = call("GET", "/tunnels")["items"]
if len(items) < 2:
    print("需要至少两条转发才能验证隔离性")
    sys.exit(1)

target = items[0]["definition"]
other = items[1]["definition"]
before_other = other["auto_start"]

call("PATCH", f"/tunnels/{target['id']}", {"auto_start": not target["auto_start"]})
after = {item["definition"]["id"]: item["definition"] for item in call("GET", "/tunnels")["items"]}

changed_self = after[target["id"]]["auto_start"] != target["auto_start"]
changed_other = after[other["id"]]["auto_start"] != before_other

print(f"{target['name']}: 自己变了={changed_self}   {other['name']}: 被连带改了={changed_other}")
print("ISOLATION:", "GREEN" if changed_self and not changed_other else "FAIL")

# 还原
call("PATCH", f"/tunnels/{target['id']}", {"auto_start": target["auto_start"]})
sys.exit(0 if changed_self and not changed_other else 1)
