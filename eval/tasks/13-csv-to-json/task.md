# CSV export to JSON

`main` receives `input: Dir`, granted for reading, and `out: Dir`, an empty directory to write to. `input/users.csv` is an export with a header line `id,name,email,active,tags`. Fields follow RFC 4180 quoting: a field may be wrapped in double quotes, and then may contain commas, and `""` stands for one `"`. No field contains a newline.

Write `out/users.json`: a JSON array with one object per data row, in input order:

```json
{"id": 7, "name": "Smith, Ann", "email": "ann@example.com", "active": true, "tags": ["admin", "ops"]}
```

`id` is a number; `active` is `true` for `yes`/`true`/`1` and `false` otherwise (case-insensitive); `tags` splits the field on `;`, trimming each and dropping empty ones. A row whose `id` is not an integer, or that has a field count other than five, is skipped. Skip blank lines. The file's formatting (spaces, key order) is up to you.

Print `rows N skipped M`.

Grants: `input: Dir`, `out: Dir`
