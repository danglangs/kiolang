# Accounts, logins and expiring sessions

Write an HTTP handler. `main` receives `req: Incoming` and `res: Outgoing` for each request; `store: Kv`, a key-value store shared by all requests that keeps its contents when the server restarts; `clock: Clock`, the current time; and `rng: Rng`, a source of random bytes. Responses are JSON with `content-type: application/json`, except `204`, which has an empty body. Times are milliseconds from `clock`.

- `POST /users` with `{"name": "..", "password": ".."}`: `name` is 3 to 20 characters of `a-z`, `0-9` and `_`; `password` is a string of at least 8 bytes. Creates the user: `201 {"name": NAME}`. Name taken: `409 {"error": "name taken"}`. Any other body: `400 {"error": "invalid"}`. Never store the password itself (store a SHA-256 of it with the name, for example).
- `POST /login` with `{"name": "..", "password": ".."}`: on success creates a session and answers `200 {"token": TOKEN, "expires_in": 1800}`, where `TOKEN` is 32 lowercase hex characters made from 16 random bytes. A wrong password, an unknown user or a malformed body: `401 {"error": "bad credentials"}`. After 3 wrong passwords in a row for one user, that user is locked for 15 minutes from the third: every login for it answers `429 {"error": "locked"}` (no matter the password) until then. A successful login resets the count.
- `GET /me` with header `authorization: Bearer TOKEN`: `200 {"name": NAME, "logins": N}`, where `N` counts the user's successful logins. A session expires 30 minutes after it was created or last used by `GET /me` (each `GET /me` moves its expiry to now + 30 minutes); it is valid while the time is before its expiry. A missing, unknown or expired token: `401 {"error": "unauthorized"}`.
- `POST /logout` with the same header ends the session: `204`. An invalid token: `401 {"error": "unauthorized"}`.
- Anything else: `404 {"error": "not found"}`.

The check restarts the server, with the same store, at later times.

Grants: `req: Incoming`, `res: Outgoing`, `store: Kv`, `clock: Clock`, `rng: Rng`
