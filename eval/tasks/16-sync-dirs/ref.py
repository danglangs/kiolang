import os

src, dst = os.environ["SRC"], os.environ["DST"]
for name in sorted(os.listdir(src), key=lambda n: n.encode()):
    data = open(os.path.join(src, name), "rb").read()
    target = os.path.join(dst, name)
    if not os.path.exists(target):
        status = "added"
    elif open(target, "rb").read() != data:
        status = "updated"
    else:
        print("same", name)
        continue
    open(target, "wb").write(data)
    print(status, name)
print("kept", len(set(os.listdir(dst)) - set(os.listdir(src))))
