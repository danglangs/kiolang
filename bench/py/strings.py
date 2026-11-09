def main():
    s = ""
    for i in range(100_000):
        s += str(i) + ","
    print(len(s))


main()
