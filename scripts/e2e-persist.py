"""The regression the user hit: config must survive a core restart.

Reads the config, restarts the core, reads it again and compares.
"""

import json
import os
import subprocess
import time
import urllib.error
import urllib.request

DATA = os.path.join(os.environ["PUBLIC"], "Threadline")
CONTROL = os.path.join(DATA, "control.json")
TEMP_CORE = os.path.join(os.environ.get("TEMP", ""), "tl-core.exe")


def read_control():
    with open(CONTROL, encoding="utf-8") as handle:
        return json.load(handle)


def call(method, path, body=None):
    control = read_control()
    base = f"http://127.0.0.1:{control['port']}/api/v1"
    headers = {"Authorization": f"Bearer {control['token']}"}
    data = None if body is None else json.dumps(body).encode()
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(request, timeout=20) as response:
        raw = response.read().decode()
        return json.loads(raw) if raw else None


def snapshot(label):
    servers = [s["name"] for s in call("GET", "/servers")["items"]]
    forwards = [t["definition"]["name"] for t in call("GET", "/tunnels")["items"]]
    print(f"{label}: tunnels={servers} forwards={forwards}")
    return sorted(servers), sorted(forwards)


def restart_core():
    subprocess.run(["taskkill", "/IM", "tl-core.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    subprocess.Popen(
        [TEMP_CORE, "--manual", "--data-dir", DATA],
        creationflags=0x00000008,  # DETACHED_PROCESS
    )
    for _ in range(20):
        time.sleep(0.5)
        try:
            call("GET", "/servers")
            return
        except (urllib.error.URLError, OSError):
            continue
    raise SystemExit("core did not come back")


subprocess.Popen(
    [TEMP_CORE, "--manual", "--data-dir", DATA],
    creationflags=0x00000008,
)
for _ in range(20):
    time.sleep(0.5)
    try:
        call("GET", "/servers")
        break
    except (urllib.error.URLError, OSError):
        continue

before = snapshot("before restart")
restart_core()
after = snapshot("after  restart")

print("PERSIST:", "GREEN" if before == after and before[0] else "FAILED")
