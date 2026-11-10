# Payments API with idempotency keys

Write an HTTP handler. `main` receives `req: Incoming` and `res: Outgoing` for each request, and `db: Db`, a SQLite database shared by all requests that keeps its contents when the server restarts (empty at first; create your schema if it is missing). Responses are JSON with `content-type: application/json`.

A payment is `{"id": "pay_N", "account": "..", "cents": N, "currency": "EUR" or "USD", "status": "..", "refunded": N}`; ids are `pay_1`, `pay_2`, .. in creation order. `status` is `pending`, then `captured` or `canceled`; a captured payment refunded in full becomes `refunded`.

- `POST /payments` with `{"account": "..", "cents": N, "currency": ".."}` (`account` a non-empty string, `cents` an integer above 0, `currency` `EUR` or `USD`, no other fields) creates a `pending` payment with `refunded` 0: `201` and the payment. Otherwise `422 {"error": "invalid payment"}`.
- `POST /payments/ID/capture`: `pending` becomes `captured`; `200` and the payment. Capturing a `captured` payment again changes nothing and answers the same. Any other status: `409 {"error": "invalid transition"}`.
- `POST /payments/ID/cancel`: `pending` becomes `canceled`, `200` and the payment; any other status `409 {"error": "invalid transition"}`.
- `POST /payments/ID/refund` with `{"cents": N}` (an integer above 0, else `422 {"error": "invalid refund"}`): only a `captured` payment (else `409 {"error": "invalid transition"}`); if `refunded + N` would exceed `cents`, `409 {"error": "exceeds captured"}`; else add `N` to `refunded` (status `refunded` when it reaches `cents`): `200` and the payment.
- `GET /payments/ID`: `200` and the payment. `GET /payments?account=A`: `200` and the account's payments, oldest first (`[]` if none).
- An unknown payment, or any other method or path: `404 {"error": "not found"}`.

**Idempotency.** `POST /payments` and `POST /payments/ID/refund` need a header `idempotency-key: K` (else `400 {"error": "missing idempotency key"}`, before anything else). The first request with a key that succeeds (2xx) is remembered with its response. A later request with the same key, the same method and path and the same JSON body (equal as JSON values: key order and spacing do not matter) gets the remembered response again, status and body exactly, and changes nothing, even after the server restarts. The same key with a different path or body: `409 {"error": "key reused"}`. A request that fails (not 2xx) is not remembered, so its key can be used again.

Grants: `req: Incoming`, `res: Outgoing`, `db: Db`
