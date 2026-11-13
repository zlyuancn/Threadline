"""量一下面板与核心之间的一次往返有多快（轮询间隔是否成为唯一瓶颈）。"""

import json
import os
import statistics
import time
import urllib.request

CONTROL = os.path.join(os.environ["PUBLIC"], "Threadline", "control.json")
with open(CONTROL, encoding="utf-8") as handle:
    control = json.load(handle)

url = f"http://127.0.0.1:{control['port']}/api/v1/tunnels"
headers = {"Authorization": f"Bearer {control['token']}"}

samples = []
for _ in range(20):
    started = time.perf_counter()
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers)) as resp:
        resp.read()
    samples.append((time.perf_counter() - started) * 1000)

print(f"GET /tunnels  n={len(samples)}")
print(f"  中位 {statistics.median(samples):.1f} ms   p95 {sorted(samples)[18]:.1f} ms   max {max(samples):.1f} ms")
print("结论：单次往返远小于轮询间隔时，延迟完全由间隔决定。")
