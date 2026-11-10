# Validate records against a JSON schema

`main` receives `input: Dir`, granted for reading.

`input/schema.json` is a schema in this small language; every schema is an object with a `type`:

| `type` | matches | other keys (all optional) |
|---|---|---|
| `"object"` | a JSON object | `properties` (field name to schema), `required` (field names), `additional` (`false`: fields not in `properties` are errors; default `true`) |
| `"array"` | an array | `items` (the schema of every element), `min_items` |
| `"string"` | a string | `min_length`, `max_length` (counted in Unicode code points, not bytes), `enum` (the allowed strings) |
| `"integer"` | a number written without fraction or exponent (`3`, not `3.0`; `true` is not an integer) | `minimum`, `maximum` |
| `"number"` | any number | `minimum`, `maximum` |
| `"boolean"`, `"null"` | `true`/`false`, `null` | |

Bounds are integers and inclusive.

`input/records.jsonl` has one record per line. Validate each against the schema and report every error, each as `PATH: MESSAGE`. The path of the record is `$`, of a field `PATH.name`, of an array element `PATH[i]` (from 0). Messages:

- `expected TYPE` (the schema's `type`) when the value has another type; nothing inside that value is checked further;
- `missing required field` at the field's path; `unexpected field` at the field's path;
- `fewer than N items` at the array's path (its elements are still checked);
- for a string, the first failing of `shorter than N`, `longer than N`, `not in enum`;
- for a number, the first failing of `less than N`, `greater than N`.

Print, per non-blank line, `line N: ok` if it has no errors, `line N: invalid JSON` if it does not parse, else one line `line N: PATH: MESSAGE` per error, sorted by `PATH` then `MESSAGE` (byte order). `N` counts every line of the file from 1. Last, print `valid V of R`, where `R` counts the non-blank lines.

Grants: `input: Dir`
