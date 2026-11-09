def main():
    xs = []
    for i in range(1_000_000):
        xs.append(i)
    k = 3
    ys = list(filter(lambda x: x % k == 0, map(lambda x: x * 2, xs)))
    print(sum(ys) + len(ys))


main()
