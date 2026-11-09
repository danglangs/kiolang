def main():
    n = 400
    mx = 100
    inside = 0
    iters = 0
    for py in range(n):
        for px in range(n):
            cx = -2.0 + 2.5 * float(px) / float(n)
            cy = -1.25 + 2.5 * float(py) / float(n)
            x = y = 0.0
            i = 0
            while i < mx and x * x + y * y <= 4.0:
                t = x * x - y * y + cx
                y = 2.0 * x * y + cy
                x = t
                i += 1
            iters += i
            if i == mx:
                inside += 1
    print(inside * 100_000_000 + iters)


main()
