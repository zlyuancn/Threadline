"""端到端检查：转发规则的 scheme 能否落库并在列表里读回。

用法：
  python scripts/check-scheme.py create   # 建一条 http 转发（用于看界面）
  python scripts/check-scheme.py clean    # 删掉它
"""

import json
import sys
import urllib.request

CONTROL = r"C:\Users\Public\Threadline\control.json"
NAME = "ui-check-http"


def api(method: str, path: str, body=None):
    control = json.load(open(CONTROL, encoding="utf-8"))
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
    with urllib.request.urlopen(req, timeout=10) as resp:
        raw = resp.read().decode()
        return json.loads(raw) if raw.strip() else None


def main() -> int:
    action = sys.argv[1] if len(sys.argv) > 1 else "create"
    tunnels = api("GET", "/tunnels")["items"]

    if action == "clean":
        for item in tunnels:
            if item["definition"]["name"] == NAME:
                api("DELETE", f"/tunnels/{item['definition']['id']}")
                print(f"deleted {NAME}")
        return 0

    servers = api("GET", "/servers")["items"]
    if not servers:
        print("no server")
        return 1

    created = api(
        "POST",
        "/tunnels",
        {
            "name": NAME,
            "server_id": servers[0]["id"],
            "local_host": "127.0.0.1",
            "local_port": 18099,
            "remote_host": "127.0.0.1",
            "remote_port": 3000,
            "auto_start": False,
            "scheme": "http",
        },
    )
    # POST 的返回体是运行态，定义要从列表读——这才是界面看到的东西。
    print("create acknowledged:", bool(created))

    listed = api("GET", "/tunnels")["items"]
    found = [i for i in listed if i["definition"]["name"] == NAME]
    print("listed scheme:", found[0]["definition"]["scheme"] if found else "MISSING")
    return 0 if found and found[0]["definition"]["scheme"] == "http" else 1


if __name__ == "__main__":
    raise SystemExit(main())
