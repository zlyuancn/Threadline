"""Print each tunnel: enabled / session state / last session error."""

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

for server in json.load(
    urllib.request.urlopen(urllib.request.Request(base + "/servers", headers=headers))
)["items"]:
    session = json.loads(
        urllib.request.urlopen(
            urllib.request.Request(
                base + f"/servers/{server['id']}/session", headers=headers
            )
        ).read()
    )
    print(
        f"{server['name']:<8} enabled={server['enabled']!s:<5} "
        f"connected={session['connected']!s:<5} error={server.get('session_error')}"
    )
