"""Remove the artefacts the probe scripts created (servers/tunnels/credentials named probe*/e2e)."""

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


def call(method, path):
    req = urllib.request.Request(base + path, data=b"" if method != "GET" else None, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read().decode()
            return resp.status, (json.loads(raw) if raw else None)
    except Exception as error:  # noqa: BLE001 - cleanup only
        return 0, str(error)


prefixes = ("probe", "e2e")

for item in call("GET", "/tunnels")[1]["items"]:
    if item["definition"]["name"].startswith(prefixes):
        print("delete tunnel", item["definition"]["name"], call("DELETE", f"/tunnels/{item['definition']['id']}")[0])

for item in call("GET", "/servers")[1]["items"]:
    if item["name"].startswith(prefixes):
        print("delete server", item["name"], call("DELETE", f"/servers/{item['id']}")[0])

for item in call("GET", "/credentials")[1]["items"]:
    if item["name"].startswith(prefixes):
        print("delete credential", item["name"], call("DELETE", f"/credentials/{item['id']}")[0])
