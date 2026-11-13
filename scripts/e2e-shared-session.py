"""Verify one SSH session is shared by several port forwards.

Checks:
1. two forwards on the same tunnel -> exactly ONE established TCP connection to the server
2. the session endpoint reports both attached
3. disconnecting the session stops every attached forward
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


def call(method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    sent = dict(headers)
    if data is not None:
        sent["Content-Type"] = "application/json"
    request = urllib.request.Request(base + path, data=data, method=method, headers=sent)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
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


def core_pids():
    """PIDs of our core processes (the user's own ssh.exe must not be counted)."""
    output = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq tl-core.exe", "/FO", "CSV", "/NH"],
        capture_output=True,
        text=True,
    ).stdout
    pids = set()
    for line in output.splitlines():
        parts = [p.strip('"') for p in line.split('","')]
        if len(parts) >= 2 and parts[1].isdigit():
            pids.add(parts[1])
    return pids


def established_to(host, port):
    """ESTABLISHED connections **owned by our core** to host:port."""
    pids = core_pids()
    output = subprocess.run(
        ["netstat", "-ano", "-p", "TCP"], capture_output=True, text=True
    ).stdout
    hits = 0
    for line in output.splitlines():
        parts = line.split()
        if len(parts) < 5 or parts[3] != "ESTABLISHED":
            continue
        if f"{host}:{port}" in parts[2] and parts[4] in pids:
            hits += 1
    return hits


cleanup()

# 两条转发挂在同一条隧道上，目标不同也没关系。
_, a = call("POST", "/tunnels", {
    "name": "share-a", "server_id": "srv_test_0001",
    "local_port": 18081, "remote_host": "1.1.1.1", "remote_port": 80,
})
_, b = call("POST", "/tunnels", {
    "name": "share-b", "server_id": "srv_test_0001",
    "local_port": 18082, "remote_host": "1.1.1.1", "remote_port": 443,
})
print("created:", a["definition"]["id"], b["definition"]["id"])

print("start a:", call("POST", f"/tunnels/{a['definition']['id']}/start")[0])
print("start b:", call("POST", f"/tunnels/{b['definition']['id']}/start")[0])
time.sleep(2)

states = {i["definition"]["name"]: i["status"]["state"] for i in forwards()}
print("states:", states)

connections = established_to("127.0.0.1", 22)
print("ESTABLISHED connections to the server:", connections)

status, session = call("GET", "/servers/srv_test_0001/session")
print("session:", status, session)

status, closed = call("POST", "/servers/srv_test_0001/session/disconnect")
print("disconnect:", status, closed)
time.sleep(1)

states_after = {
    i["definition"]["name"]: i["status"]["state"]
    for i in forwards()
    if i["definition"]["name"].startswith("share-")
}
print("states after disconnect:", states_after)
print("connections after:", established_to("127.0.0.1", 22))

expected_attached = len(
    [i for i in forwards() if i["definition"]["server_id"] == "srv_test_0001"]
)

ok = (
    states.get("share-a") == "running"
    and states.get("share-b") == "running"
    and connections == 1
    and session["connected"] is True
    and session["attached_forwards"] == expected_attached
    and closed["stopped_forwards"] == expected_attached
    and all(state != "running" for state in states_after.values())
)
cleanup()
print("SHARED SESSION:", "GREEN" if ok else "FAILED")
