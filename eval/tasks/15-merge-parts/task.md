# Merge sorted parts

`main` receives `input: Dir`, granted for reading, and `out: Dir`, an empty directory to write to. A batch job left its results in `input` as files named `part-NNNN.txt`, each holding integers one per line, sorted ascending. Other files in `input` are not parts and must be ignored.

Write `out/merged.txt`: every integer from all the parts, sorted ascending, duplicates kept once, one per line, each line ending in a newline. Blank lines in a part are ignored; a part with a line that is not an integer is corrupt and is left out entirely.

Print `parts N corrupt M values V`: the number of part files, how many of them were corrupt, and the number of lines written.

Grants: `input: Dir`, `out: Dir`
