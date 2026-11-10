# Revenue report from a read-only database

`main` receives `shop: Db`, a SQLite database granted read-only. Its schema:

```sql
CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, country TEXT NOT NULL);
CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id),
                     placed TEXT NOT NULL,          -- 'YYYY-MM-DD'
                     status TEXT NOT NULL,          -- 'paid', 'refunded' or 'pending'
                     total_cents INTEGER NOT NULL);
```

Print a per-country report of paid orders placed in 2026:

```
COUNTRY customers=C orders=N revenue=CENTS top=NAME
```

where `customers` counts the country's customers with at least one such order, `orders` and `revenue` count and sum those orders, and `top` is the country's customer with the highest paid 2026 revenue (equal revenue: the smaller customer id). Countries without such orders are left out. Order by revenue, highest first, then country name.

Then print `total orders=N revenue=CENTS` over all countries.

Grants: `shop: Db` (read-only)
