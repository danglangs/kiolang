# Index a directory tree

`main` receives `input: Dir`, granted for reading, and `out: Dir`, an empty directory to write to. `input` holds text files and subdirectories, nested to any depth.

Write `out/index.csv`: a header line `path,bytes,lines`, then one line per file anywhere under `input`, with its path relative to `input` (`docs/api/v1.md`), its size in bytes and its number of lines (the count of newline characters, plus one if the file is non-empty and does not end in a newline). Sort the lines by path (byte order). Every line ends in a newline.

Print `files N bytes TOTAL`, the number of files and the sum of their sizes.

Grants: `input: Dir`, `out: Dir`
