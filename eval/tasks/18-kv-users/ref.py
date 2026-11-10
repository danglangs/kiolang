import json
import os

store = os.environ["USERS"]
users = json.load(open(store)) if os.path.exists(store) else {}
for line in open(os.path.join(os.environ["INPUT"], "commands.txt")):
    f = line.split()
    if not f:
        continue
    cmd, args = f[0], f[1:]
    if cmd == "put" and len(args) == 3:
        users[args[0]] = {"id": args[0], "name": args[1], "email": args[2]}
        print("ok", args[0])
    elif cmd in ("rename", "get", "del") and len(args) == (2 if cmd == "rename" else 1):
        uid = args[0]
        if uid not in users:
            print("missing", uid)
        elif cmd == "rename":
            users[uid]["name"] = args[1]
            print("ok", uid)
        elif cmd == "get":
            print(json.dumps(users[uid], separators=(",", ":")))
        else:
            del users[uid]
            print("ok", uid)
    elif cmd == "count" and not args:
        print("count", len(users))
    else:
        print("bad", line.strip())
json.dump(users, open(store, "w"))
