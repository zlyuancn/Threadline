"""Verify a tunnel that cannot connect reports WHY, not just "未连接"."""

import json
import os
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


def call(method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    sent = dict(headers)
    if data is not None:
        sent["Content-Type"] = "application/json"
    request = urllib.request.Request(base + path, data=data, method=method, headers=sent)
    try:
        with urllib.request.urlopen(request, timeout=40) as response:
            raw = response.read().decode()
            return response.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode(errors="replace")


for item in call("GET", "/servers")[1]["items"]:
    if item["name"] == "dead-tunnel":
        call("DELETE", f"/servers/{item['id']}")

status, created = call("POST", "/servers", {
    "name": "dead-tunnel",
    "host": "127.0.0.1",
    "port": 1,  # 关着的端口：必然连不上
    "username": "root",
    "kind": "password",
    "secret": "x",
})
print("create:", status)

call("POST", f"/servers/{created['id']}/enabled", {"enabled": True})

import time

error = None
for _ in range(10):
    time.sleep(1)
    item = [
        s
        for s in call("GET", "/servers")[1]["items"]
        if s["id"] == created["id"]
    ][0]
    error = item.get("session_error")
    if error:
        break

print("session_error:", error)
ok = bool(error) and ("127.0.0.1" in error or "1" in error)
call("POST", f"/servers/{created['id']}/enabled", {"enabled": False})
call("DELETE", f"/servers/{created['id']}")
print("SESSION ERROR:", "GREEN" if ok else "FAILED")
