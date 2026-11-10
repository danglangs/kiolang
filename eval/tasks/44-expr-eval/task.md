# An expression calculator

`main` receives `input: Dir`, granted for reading.

Each line of `input/program.txt` is a statement in this grammar (tokens may be separated by any number of spaces; blank lines are skipped):

```
line   := "let" NAME "=" expr | expr
expr   := term (("+" | "-") term)*
term   := factor (("*" | "/" | "%") factor)*
factor := "-" factor | power
power  := atom ("^" factor)?
atom   := INT | NAME | NAME "(" expr ("," expr)* ")" | "(" expr ")"
```

`INT` is one or more ASCII digits; `NAME` is an ASCII letter or `_` followed by ASCII letters, digits and `_`. A line starting with the name `let` is a `let` statement. So `^` binds tighter than unary minus (`-2 ^ 2` is -4), is right-associative (`2 ^ 3 ^ 2` is 512), and its exponent may be negated (`2 ^ -1`).

Values are 64-bit signed integers. `/` and `%` truncate toward zero (`-7 / 2` is -3, `-7 % 3` is -1, `7 % -3` is 1). `a ^ b` is `a` to the power `b` (`0 ^ 0` is 1). The functions are `min(a, b)`, `max(a, b)` and `abs(a)`. `let` binds a variable for the following lines (a `let` that fails binds nothing); variables may be rebound.

Evaluate operands left to right; the first error wins. Errors:

| Message | When |
|---|---|
| `syntax error` | the line does not match the grammar (checked before anything is evaluated) |
| `unknown variable NAME` | |
| `unknown function NAME`, `wrong number of arguments to NAME` | checked before the call's arguments are evaluated |
| `division by zero` | `/` or `%` by 0 |
| `negative exponent` | |
| `overflow` | a literal or any result outside -9223372036854775808 to 9223372036854775807 |

For each non-blank line `N` (from 1), print `N: VALUE` for an expression, `N: NAME = VALUE` for a `let`, or `N: error: MESSAGE`.

Grants: `input: Dir`
