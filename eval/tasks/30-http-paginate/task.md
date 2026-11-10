# Page through orders and post a summary

`main` receives `api: Http`, granted GET and POST access to an orders JSON API.

- `GET /orders?page=N` (pages start at 1) returns `{"orders": [{"id": 17, "status": "paid", "cents": 1250, "currency": "EUR"}, ..], "next": M}`, where `next` is the following page number, or `null` on the last page.
- `POST /summaries` with a JSON body creates a summary and answers `201 {"id": "sum-..."}`.

Read every page. Then POST to `/summaries`, with header `content-type: application/json`, the body

```json
{"orders": N, "paid_cents": {"EUR": .., "USD": ..}, "refunded": R, "largest": ID}
```

where `orders` counts all orders, `paid_cents` sums `cents` of orders with status `paid` per currency (only currencies that have paid orders), `refunded` counts orders with status `refunded`, and `largest` is the id of the paid order with the most cents (equal cents: the smaller id).

Print the summary id the API returns. If the POST does not answer 201, fail (exit non-zero).

Grants: `api: Http` (GET, POST)
