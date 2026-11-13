"""Verify port forwards cannot run while their tunnel is disabled."""

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
    return sum(
        1
        for line in netstat.splitlines()
        for parts in [line.split()]
        if len(parts) >= 5
        and parts[3] == "ESTABLISHED"
        and f"{host}:{port}" in parts[2]
        and parts[4] in pids
    )


# 确保隧道是关闭状态。
call("POST", f"/servers/{SERVER}/enabled", {"enabled": False})
time.sleep(1)

target = [i for i in forwards() if i["definition"]["name"] == "homeassistant"][0]
status, body = call("POST", f"/tunnels/{target['definition']['id']}/start")
print("start while tunnel disabled:", status)
print("  body:", str(body)[:120])

after = [i for i in forwards() if i["definition"]["name"] == "homeassistant"][0]
print("  state:", after["status"]["state"], "| last_error:", after["status"]["last_error"])
print("  core connections:", core_connections("127.0.0.1", 22))

# 打开隧道后再试：应该能跑起来。
print("--- enable tunnel ---")
call("POST", f"/servers/{SERVER}/enabled", {"enabled": True})
time.sleep(3)
resumed = [i for i in forwards() if i["definition"]["name"] == "homeassistant"][0]
print("  state:", resumed["status"]["state"])
print("  core connections:", core_connections("127.0.0.1", 22))

ok = (
    status == 409
    and "未启用" in str(body)
    and after["status"]["state"] != "running"
    and "隧道未启用" in (after["status"]["last_error"] or "")
    and resumed["status"]["state"] == "running"
)
call("POST", f"/servers/{SERVER}/enabled", {"enabled": False})
print("GATE:", "GREEN" if ok else "FAILED")
