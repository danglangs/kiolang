# One-way directory sync

`main` receives `src: Dir`, granted for reading, and `dst: Dir`, granted for reading and writing. Both hold files only (no subdirectories).

Make `dst` a backup of `src`: copy every file of `src` that is missing from `dst` or whose contents differ. Never delete or change a file that exists only in `dst`.

Print one line per file of `src`, by name (byte order): `added NAME`, `updated NAME` or `same NAME`. Then print `kept N` for the number of files only in `dst`.

Grants: `src: Dir`, `dst: Dir`
