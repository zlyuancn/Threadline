"""Verify: enabling a tunnel without a trusted key surfaces a pending fingerprint
that the UI can show, and accepting it lets the next attempt proceed."""

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
    if item["name"] == "trust-test":
        call("DELETE", f"/servers/{item['id']}")

_, server = call("POST", "/servers", {
    "name": "trust-test",
    "host": "127.0.0.1",
    "port": 22,
    "username": "user",
    "kind": "password",
    "secret": "wrong-on-purpose",
})
sid = server["id"]

status, body = call("POST", f"/servers/{sid}/enabled", {"enabled": True})
print("enable without trust:", status)
print("  message:", (body.get("error", {}).get("message") if isinstance(body, dict) else body))

_, view = call("GET", f"/servers/{sid}/hostkey")
print("pending visible:", bool(view.get("pending_fingerprint")), view.get("pending_fingerprint"))

status, _ = call("POST", f"/servers/{sid}/hostkey/accept", {"reject": False})
print("accept:", status)

status, body = call("POST", f"/servers/{sid}/enabled", {"enabled": True})
print("enable after trust:", status)
print("  message:", (body.get("error", {}).get("message") if isinstance(body, dict) else body))

_, view = call("GET", f"/servers/{sid}/hostkey")
print("trusted now:", bool(view.get("trusted_fingerprint")))
call("DELETE", f"/servers/{sid}")
print("cleanup ok:", not [s for s in call("GET", "/servers")[1]["items"] if s["name"] == "trust-test"])
