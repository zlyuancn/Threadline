"""验证隧道排序接口：倒序提交后，列表按新顺序返回。"""

import json
import os
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

base = f"http://127.0.0.1:{control['port']}/api/v1"
headers = {
    "Authorization": f"Bearer {control['token']}",
    "Content-Type": "application/json",
}


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(req, timeout=15) as resp:
        raw = resp.read().decode()
        return json.loads(raw) if raw.strip() else None


# 预检：界面是跨源调的，PUT 必须先过 OPTIONS。
# 允许方法列表里漏掉 PUT 时，浏览器报 "Failed to fetch"，而脚本直连却一切正常。
preflight = urllib.request.Request(
    base + "/servers/order",
    method="OPTIONS",
    headers={
        **headers,
        "Origin": "http://tauri.localhost",
        "Access-Control-Request-Method": "PUT",
        "Access-Control-Request-Headers": "authorization,content-type",
    },
)
with urllib.request.urlopen(preflight, timeout=10) as resp:
    allowed = resp.headers.get("access-control-allow-methods", "")
print("preflight allow-methods:", allowed)
print("PREFLIGHT:", "GREEN" if "PUT" in allowed.upper() else "FAIL")

items = call("GET", "/servers")["items"]
print("before:", [item["name"] for item in items])
if len(items) < 2:
    print("只有一个隧道，顺序变化不可见（接口仍会成功）")

ids = [item["id"] for item in items]
reversed_ids = list(reversed(ids))
call("PUT", "/servers/order", {"ids": reversed_ids})
after = [item["id"] for item in call("GET", "/servers")["items"]]
print("after :", after)
print("REORDER:", "GREEN" if after == reversed_ids else "FAIL")

# 还原，别把用户顺序留在倒序上
call("PUT", "/servers/order", {"ids": ids})
print("restored:", [item["name"] for item in call("GET", "/servers")["items"]])
