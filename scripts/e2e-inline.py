"""Verify the merged model: a tunnel carries its own login material (no credential step)."""

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
    if item["name"] == "inline-test":
        call("DELETE", f"/servers/{item['id']}")

# 只给用户名和密码，不再有 credential_id
status, created = call("POST", "/servers", {
    "name": "inline-test",
    "host": "127.0.0.1",
    "port": 22,
    "username": "root",
    "kind": "password",
    "secret": "definitely-not-the-password",
})
print("create tunnel:", status, created if status >= 400 else {
    "id": created["id"], "username": created["username"], "enabled": created["enabled"]
})

if status < 400:
    # 探测应该能连上、并在**认证**阶段失败——说明登录材料确实被内嵌并使用。
    status, report = call("POST", f"/servers/{created['id']}/probe")
    print("probe:", report["stage"], "|", report["message"][:70])

    # 端口挂在这条隧道下
    status, forward = call("POST", "/tunnels", {
        "name": "inline-fwd",
        "server_id": created["id"],
        "local_port": 18099,
        "remote_host": "1.1.1.1",
        "remote_port": 80,
        "auto_start": True,
    })
    print("create forward:", status)
    call("DELETE", f"/tunnels/{forward['definition']['id']}")
    call("DELETE", f"/servers/{created['id']}")

leftovers = [s["name"] for s in call("GET", "/servers")[1]["items"] if s["name"] == "inline-test"]
print("cleanup ok:", not leftovers)
