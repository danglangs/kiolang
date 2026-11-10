# Word frequencies

`main` receives `input: Dir`, granted for reading. `input/text.txt` is plain English text.

Print the five most frequent words, one per line, as `word count`. A word is a maximal run of ASCII letters (`a`-`z`, `A`-`Z`), lowercased; everything else separates words. Order by count, highest first; equal counts alphabetically.

Grants: `input: Dir`

Example: for `The cat and the hat. THE end, and cat!` print

```
the 3
and 2
cat 2
end 1
hat 1
```
