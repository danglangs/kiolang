# Quoted CSV: clean it and total it

`main` receives `input: Dir`, granted for reading, and `out: Dir`, granted for writing (empty).

`input/sales.csv` is CSV as in RFC 4180: records end with CRLF or LF; a field may be quoted with `"`, and then may contain commas, CR, LF and doubled quotes (`""` for one `"`). A quote may only appear inside a quoted field (doubled) or as the whole field's delimiters: a field such as `"a"b` or `a"b` makes its record malformed. A quoted field still open at the end of the file makes that last record malformed. An empty line between records is skipped. The first record is the header `region,product,qty,price,note`; records after it are numbered from 1 (skipped empty lines do not count).

A record is valid if it has exactly 5 fields, `region` and `product` are not empty, `qty` is an integer (negative for a return), and `price` is digits, optionally followed by `.` and one or two digits (`10`, `2.5`, `0.99`). Money is exact: work in cents.

Write `out/clean.csv`: the header, then every valid record in input order, with `price` written with exactly two decimals (`2.5` becomes `2.50`, `10` becomes `10.00`) and every other field as it was. A field is written quoted (with inner quotes doubled) exactly when it contains a comma, a quote, CR or LF. Every record, the header included, ends with LF.

Print, for each region with valid records, sorted by name (byte order):

`REGION revenue R items Q top PRODUCT`

where `R` is the sum of `qty * price` as a decimal with two places (`-1.50` when negative), `Q` the sum of `qty`, and `PRODUCT` the product with the largest revenue in that region (equal: the smaller name, byte order). Last, `records N bad B: R1, R2, ..`, listing the numbers of invalid records (`records N bad 0` when none).

Grants: `input: Dir`, `out: Dir`
