def main():
    s = 0
    for i in range(30_000_000):
        s = (s * 31 + i) % 1_000_003
    print(s)


main()
