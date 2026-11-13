"""检查删除隧道时的拦截文案（应说清还剩几条转发、叫什么）。"""

import json
import os
import urllib.error
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")

with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
headers = {"Authorization": f"Bearer {control['token']}"}

servers = json.load(
    urllib.request.urlopen(urllib.request.Request(base + "/servers", headers=headers))
)["items"]
if not servers:
    raise SystemExit("no server")

req = urllib.request.Request(f"{base}/servers/{servers[0]['id']}", method="DELETE", headers=headers)
try:
    with urllib.request.urlopen(req, timeout=10) as resp:
        print("unexpected success:", resp.status)
except urllib.error.HTTPError as err:
    body = json.loads(err.read().decode())
    print("status:", err.code)
    print("body:", json.dumps(body, ensure_ascii=False))
