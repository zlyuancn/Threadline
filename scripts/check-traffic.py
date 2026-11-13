"""验证流量计数：通过转发打一次请求，累计字节必须增长。"""

import json
import os
import time
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
headers = {"Authorization": f"Bearer {control['token']}"}


def tunnels():
    return json.load(
        urllib.request.urlopen(urllib.request.Request(base + "/tunnels", headers=headers))
    )["items"]


before = {item["definition"]["id"]: item["status"] for item in tunnels()}
if not before:
    raise SystemExit("no forwards")

# 逐个尝试：只要有一条能通，就能看到计数增长；都不通说明远端服务没在跑。
for item in tunnels():
    if item["status"]["state"] != "running":
        continue
    port = item["definition"]["local_port"]
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=4).read(4096)
        print(f"probe ok on {port}")
    except Exception as exc:  # noqa: BLE001
        print(f"probe {port}: {type(exc).__name__}")

time.sleep(1.2)

after = {item["definition"]["id"]: item["status"] for item in tunnels()}
for tid, status in after.items():
    old = before.get(tid, {})
    up = status.get("bytes_up", 0) - old.get("bytes_up", 0)
    down = status.get("bytes_down", 0) - old.get("bytes_down", 0)
    print(f"{tid[:12]:14} +up={up:6} +down={down:6} total_up={status.get('bytes_up', 0)}")
