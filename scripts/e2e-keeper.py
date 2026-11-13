"""Auto-recovery: an enabled tunnel must come back after a restart and after a drop."""

import json
import os
import subprocess
import time
import urllib.error
import urllib.request

DATA = os.path.join(os.environ["PUBLIC"], "Threadline")
CONTROL = os.path.join(DATA, "control.json")
TEMP_CORE = os.path.join(os.environ.get("TEMP", ""), "tl-core.exe")


def call(method, path, body=None):
    with open(CONTROL, encoding="utf-8") as handle:
        control = json.load(handle)
    base = f"http://127.0.0.1:{control['port']}/api/v1"
    headers = {"Authorization": f"Bearer {control['token']}"}
    data = None if body is None else json.dumps(body).encode()
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(request, timeout=20) as response:
        raw = response.read().decode()
        return json.loads(raw) if raw else None


def server_id(name="dev-a"):
    return next(s["id"] for s in call("GET", "/servers")["items"] if s["name"] == name)


def connected(sid):
    return call("GET", f"/servers/{sid}/session")["connected"]


def wait_connected(sid, seconds):
    for _ in range(seconds * 2):
        if connected(sid):
            return True
        time.sleep(0.5)
    return False


def start_core():
    subprocess.Popen(
        [TEMP_CORE, "--manual", "--data-dir", DATA],
        creationflags=0x00000008,
    )
    for _ in range(30):
        time.sleep(0.5)
        try:
            call("GET", "/servers")
            return
        except (urllib.error.URLError, OSError):
            continue
    raise SystemExit("core did not start")


sid = server_id()
print("tunnel:", sid, "enabled:", call("GET", "/servers")["items"][0]["enabled"])

print("--- 模拟核心重启 ---")
subprocess.run(["taskkill", "/IM", "tl-core.exe", "/F"], capture_output=True)
time.sleep(1.5)
start_core()
recovered = wait_connected(sid, 20)
print("重启后自动连上:", recovered)

print("--- 模拟会话被断开 ---")
call("POST", f"/servers/{sid}/session/disconnect")
print("断开后 connected:", connected(sid))
again = wait_connected(sid, 25)
print("守护自动重连:", again)

print("KEEPER:", "GREEN" if recovered and again else "FAILED")
