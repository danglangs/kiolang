# Roman numerals both ways

`main` receives `input: Dir`, granted for reading. `input/numbers.txt` has one value per line: either a decimal integer or a Roman numeral (uppercase letters `IVXLCDM`).

For each non-blank line (trimmed), print one line `VALUE = CONVERTED`:

- an integer from 1 to 3999 becomes its standard Roman numeral (`1994 = MCMXCIV`);
- a Roman numeral in standard form becomes its integer (`MCMXCIV = 1994`). Standard form is what the first rule produces: `IIII`, `IC` or `MMMM` are not standard;
- anything else prints `VALUE = invalid`.

Grants: `input: Dir`

Example: for

```
1994
XLII
0
IIII
```

print

```
1994 = MCMXCIV
XLII = 42
0 = invalid
IIII = invalid
```
