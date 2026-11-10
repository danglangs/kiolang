# Bracket checker

`main` receives `input: Dir`, granted for reading. `input/snippets.txt` has one code snippet per line.

Check each line's brackets `()`, `[]` and `{}`; every other character is ignored. Print one line per input line, in order (blank lines included):

- `ok` when every bracket is closed by its partner in the right order;
- `col N: expected X, found Y` at the first closing bracket that does not match the innermost open one (`X` is the closer that was expected, `Y` the one found; `N` counts characters from 1);
- `col N: unexpected Y` for a closing bracket with nothing open;
- `unclosed X` when the line ends with brackets still open, `X` the closer the innermost one needs.

Lines are ASCII.

Grants: `input: Dir`

Example: for

```
f(a[1], {b: 2})
f(a[1)]
x)
{[(
```

print

```
ok
col 6: expected ], found )
col 2: unexpected )
unclosed )
```
