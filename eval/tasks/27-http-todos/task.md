# Todo JSON API

Write an HTTP handler. `main` receives `req: Incoming` and `res: Outgoing` for each request, and `store: Kv`, a key-value store shared by all requests. Responses with a body are JSON with `content-type: application/json`.

A todo is `{"id": N, "title": "..", "done": false}`; ids are 1, 2, 3, .. in creation order and never reused.

- `POST /todos` with body `{"title": ".."}` creates a todo: `201` and the todo. A body that is not a JSON object with a non-empty string `title` (after trimming; store it trimmed) answers `400 {"error": "title required"}`.
- `GET /todos` answers `200` and the array of all todos, by id. `GET /todos?done=true` or `?done=false` filters by `done`.
- `GET /todos/ID` answers `200` and the todo.
- `PATCH /todos/ID` with a body that is a JSON object with any of `title` (a string, non-empty after trimming; stored trimmed) and `done` (boolean) updates those fields: `200` and the todo. A body that is not such an object, or a field of the wrong type, answers `400 {"error": "bad update"}` and changes nothing.
- `DELETE /todos/ID` deletes it: `204` with an empty body.
- An `ID` that is not an existing todo's id answers `404 {"error": "not found"}`; so does any other method or path.

Grants: `req: Incoming`, `res: Outgoing`, `store: Kv`
