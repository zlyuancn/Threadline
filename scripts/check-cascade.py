"""验证级联：隧道连上时，新建/修改"启用"的转发会自动连上；关掉它会停。

用法：python scripts/check-cascade.py
"""

import json
import os
import sys
import time
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
NAME = "cascade-check"


def call(method, path, body=None):
    with open(CONTROL, encoding="utf-8") as handle:
        control = json.load(handle)
    base = f"http://127.0.0.1:{control['port']}/api/v1"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        base + path,
        data=data,
        method=method,
        headers={
            "Authorization": f"Bearer {control['token']}",
            "Content-Type": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=20) as resp:
        raw = resp.read().decode()
        return json.loads(raw) if raw.strip() else None


def state_of(tunnel_id):
    for item in call("GET", "/tunnels")["items"]:
        if item["definition"]["id"] == tunnel_id:
            return item["status"]["state"]
    return "missing"


def wait_state(tunnel_id, want, seconds=8.0):
    deadline = time.time() + seconds
    last = None
    while time.time() < deadline:
        last = state_of(tunnel_id)
        if last == want:
            return True
        time.sleep(0.4)
    print(f"  expected {want}, got {last}")
    return False


def main():
    servers = call("GET", "/servers")["items"]
    if not servers:
        print("no server")
        return 1

    for item in call("GET", "/tunnels")["items"]:
        if item["definition"]["name"] == NAME:
            call("DELETE", f"/tunnels/{item['definition']['id']}")

    ok = True

    created = call(
        "POST",
        "/tunnels",
        {
            "name": NAME,
            "server_id": servers[0]["id"],
            "local_host": "127.0.0.1",
            "local_port": 18097,
            "remote_host": "127.0.0.1",
            "remote_port": 16100,
            "auto_start": True,
            "scheme": "tcp",
        },
    )
    tunnel_id = created["definition"]["id"]
    ok &= wait_state(tunnel_id, "running")
    print("create(auto_start) -> running:", ok)

    # 改配置：应当停掉旧监听、按新配置重连，而不是停在 stopped
    call("PATCH", f"/tunnels/{tunnel_id}", {"remote_port": 16101})
    started = wait_state(tunnel_id, "running")
    ok &= started
    print("patch(remote_port) -> running:", started)

    call("PATCH", f"/tunnels/{tunnel_id}", {"auto_start": False})
    stopped = wait_state(tunnel_id, "stopped")
    ok &= stopped
    print("patch(auto_start=false) -> stopped:", stopped)

    call("DELETE", f"/tunnels/{tunnel_id}")
    print("CASCADE:", "GREEN" if ok else "RED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
