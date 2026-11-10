# Notes API on SQLite

Write an HTTP handler. `main` receives `req: Incoming` and `res: Outgoing` for each request, and `db: Db`, a SQLite database shared by all requests (empty at start; create your schema if it is missing). Responses are JSON with `content-type: application/json`.

A note is `{"id": N, "title": "..", "body": "..", "tags": ["..", ..]}`; ids are assigned 1, 2, 3, .. and never reused, even after a delete.

- `POST /notes` with a JSON object body `{"title": "..", "body": "..", "tags": [..]}` creates a note and answers `201 {"id": N}`. `title` must be a non-empty string, `body` a string, `tags` an array of strings (optional, default `[]`; stored lowercased, without duplicates, in first-seen order). Otherwise `400 {"error": "invalid note"}`.
- `GET /notes/ID` answers `200` and the note.
- `GET /notes` answers `200` and `[{"id": N, "title": ".."}, ..]` for all notes, newest (highest id) first. `GET /notes?tag=T` lists only notes with tag `T` (lowercased); `?q=WORD` only notes whose title or body contains `WORD` (case-sensitive). Both may be given (`?tag=work&q=plan`, any order); values contain only letters, digits and `-`.
- `DELETE /notes/ID` deletes it: `200 {"deleted": ID}`.
- An unknown `ID`, a malformed path or any other method answers `404 {"error": "not found"}`.

Grants: `req: Incoming`, `res: Outgoing`, `db: Db`
