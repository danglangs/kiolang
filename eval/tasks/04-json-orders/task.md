# Paid orders per customer

`main` receives `input: Dir`, granted for reading. `input/orders.json` is a JSON array of orders:

```json
[{"id": 1, "customer": "acme", "status": "paid",
  "items": [{"sku": "A-1", "qty": 2, "price_cents": 350}]}]
```

`status` is `"paid"`, `"refunded"` or `"pending"`. An order's total is the sum of `qty * price_cents` over its items (an order may have no items).

Print one line per customer with at least one paid order, `customer ORDERS TOTAL_CENTS`, counting paid orders only, ordered by customer name compared byte by byte (uppercase before lowercase). Then print `refunded N TOTAL_CENTS` for all refunded orders, and `pending N` for the number of pending ones.

Grants: `input: Dir`

Example: for the one order above print

```
acme 1 700
refunded 0 0
pending 0
```
