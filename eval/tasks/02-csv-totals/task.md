# Sales totals by region

`main` receives `input: Dir`, granted for reading. `input/sales.csv` has a header line and then rows `date,region,product,qty,unit_cents`, for example `2026-10-01,west,widget,3,1250`.

Print one line per region, `region qty revenue`, where `qty` is the sum of quantities and `revenue` is the sum of `qty * unit_cents` written in dollars with two decimals (`1234.50`). Order by revenue, highest first; equal revenue by region name. Skip blank lines. A row that does not have five fields, or whose `qty` or `unit_cents` is not an integer, is skipped and counted; after the regions print `skipped N`. Fields may have spaces around them: trim them.

Grants: `input: Dir`

Example: for

```
date,region,product,qty,unit_cents
2026-10-01,west,widget,3,1250
2026-10-01,east,gadget,1,999
2026-10-02,west,gadget,x,999
```

print

```
west 3 37.50
east 1 9.99
skipped 1
```
