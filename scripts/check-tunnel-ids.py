"""检查转发 id 是否唯一（老版本会因 slug + pid 撞车）。"""

import json
import os
import sys

path = os.path.join(os.environ["PUBLIC"], "Threadline", "tunnels.json")
with open(path, encoding="utf-8") as handle:
    data = json.load(handle)

ids = [item["id"] for item in data["items"]]
for item in data["items"]:
    print(f'{item["id"]:24} {item["name"]}')

print(f"\n共 {len(ids)} 条，唯一 id {len(set(ids))} 个")
print("IDS:", "GREEN" if len(ids) == len(set(ids)) else "FAIL")
sys.exit(0 if len(ids) == len(set(ids)) else 1)
