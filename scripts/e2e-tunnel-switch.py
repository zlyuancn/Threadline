"""Verify the tunnel-level enable switch.

1. tunnel enabled -> session comes up, enabled forwards are restored
2. tunnel disabled -> session gone and every attached forward stops,
   but each forward keeps its own intent (auto_start stays true)
"""

import json
import os
import subprocess
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
SERVER = "srv_test_0001"


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


def forwards():
    return call("GET", "/tunnels")[1]["items"]


def cleanup():
    for item in forwards():
        if item["definition"]["name"].startswith("share-"):
            call("DELETE", f"/tunnels/{item['definition']['id']}")


def core_connections(host, port):
    output = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq tl-core.exe", "/FO", "CSV", "/NH"],
        capture_output=True,
        text=True,
    ).stdout
    pids = {
        parts[1].strip('"')
        for parts in (line.split('","') for line in output.splitlines())
        if len(parts) >= 2 and parts[1].strip('"').isdigit()
    }
    netstat = subprocess.run(
        ["netstat", "-ano", "-p", "TCP"], capture_output=True, text=True
    ).stdout
    hits = 0
    for line in netstat.splitlines():
        parts = line.split()
        if len(parts) >= 5 and parts[3] == "ESTABLISHED" and f"{host}:{port}" in parts[2]:
            if parts[4] in pids:
                hits += 1
    return hits


cleanup()

# 两条转发，都标记为"启用"（随隧道恢复）。
for name, port in (("share-a", 18081), ("share-b", 18082)):
    call("POST", "/tunnels", {
        "name": name, "server_id": SERVER,
        "local_port": port, "remote_host": "1.1.1.1", "remote_port": 80,
        "auto_start": True,
    })

print("--- 打开隧道 ---")
status, opened = call("POST", f"/servers/{SERVER}/enabled", {"enabled": True})
print("enable:", status, opened)
time.sleep(2)
states = {i["definition"]["name"]: i["status"]["state"] for i in forwards()}
print("states:", states)
print("core connections:", core_connections("127.0.0.1", 22))

print("--- 关闭隧道 ---")
status, closed = call("POST", f"/servers/{SERVER}/enabled", {"enabled": False})
print("disable:", status, closed)
time.sleep(1)
states_off = {i["definition"]["name"]: i["status"]["state"] for i in forwards()}
intents = {i["definition"]["name"]: i["definition"]["auto_start"] for i in forwards()}
print("states:", states_off)
print("forward intent kept:", intents)
print("core connections:", core_connections("127.0.0.1", 22))
server = [s for s in call("GET", "/servers")[1]["items"] if s["id"] == SERVER][0]
print("tunnel enabled flag:", server["enabled"])

ok = (
    opened["session_connected"] is True
    and states.get("share-a") == "running"
    and states.get("share-b") == "running"
    and closed["affected_forwards"] >= 2
    and all(state != "running" for state in states_off.values())
    and intents.get("share-a") is True
    and intents.get("share-b") is True
    and server["enabled"] is False
    and core_connections("127.0.0.1", 22) == 0
)
cleanup()
print("TUNNEL SWITCH:", "GREEN" if ok else "FAILED")
