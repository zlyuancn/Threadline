"""Probe every configured tunnel and print the stage/message the app would show."""

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

servers = json.load(
    urllib.request.urlopen(urllib.request.Request(base + "/servers", headers=headers))
)["items"]

for server in servers:
    request = urllib.request.Request(
        base + f"/servers/{server['id']}/probe",
        data=b"",
        method="POST",
        headers=headers,
    )
    report = json.loads(urllib.request.urlopen(request, timeout=30).read().decode())
    print(f"{server['name']:<10} {server['host']}:{server['port']} -> {report['stage']}")
    print(f"           {report['message']}")
