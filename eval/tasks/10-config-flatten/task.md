# Flatten and merge config

`main` receives `input: Dir`, granted for reading. It holds two JSON objects: `input/base.json` and `input/override.json`.

Merge them: start from `base`; for each key of `override`, if both values are objects merge them the same way (recursively), otherwise the override's value replaces the base's (an array replaces an array whole). A `null` in the override removes the key.

Then print every leaf of the merged object as `path=value`, one per line, sorted by path (byte order). The path joins object keys and array indexes with `.` (`server.ports.0`). Values print as JSON: strings quoted (`"x"`), numbers, `true`, `false`. An empty object or array is itself a leaf and prints `{}` or `[]`. Numbers are integers.

Grants: `input: Dir`

Example: base `{"a": {"b": 1, "c": [1, 2]}, "d": "x"}` and override `{"a": {"c": [3]}, "d": null, "e": {}}` print

```
a.b=1
a.c.0=3
e={}
```
