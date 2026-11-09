def main():
    n = 1_000_000
    xs = []
    x = 12345
    for i in range(n):
        x = (x * 1103515245 + 12345 + i) % 2147483648
        xs.append(x)
    xs.sort()
    a = 0
    for i in range(len(xs)):
        a += xs[i] % 1000
    b = 0
    for v in xs:
        b += v % 7
    print(a + b + xs[n // 2])


main()
