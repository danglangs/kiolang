import heapq
import os

adj, trips = {}, []
for line in open(os.path.join(os.environ["INPUT"], "routes.txt")):
    f = line.split()
    if not f or f[0].startswith("#"):
        continue
    if f[0] == "road":
        adj.setdefault(f[1], []).append((f[2], int(f[3])))
        adj.setdefault(f[2], []).append((f[1], int(f[3])))
    elif f[0] == "trip":
        trips.append((f[1], f[2]))

for a, b in trips:
    # Dijkstra on (cost, path): the smallest path list wins among equal costs.
    heap, done = [(0, [a])], set()
    found = None
    while heap:
        cost, path = heapq.heappop(heap)
        town = path[-1]
        if town in done:
            continue
        done.add(town)
        if town == b:
            found = (cost, path)
            break
        for nxt, c in adj.get(town, []):
            if nxt not in done:
                heapq.heappush(heap, (cost + c, path + [nxt]))
    if found:
        print(" -> ".join(found[1]), found[0])
    else:
        print(a, b, "unreachable")
