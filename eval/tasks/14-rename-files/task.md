# Normalize file names

`main` receives `input: Dir`, granted for reading, and `out: Dir`, an empty directory to write to. `input` holds files (no subdirectories) with untidy names such as `Q3 Report_FINAL.TXT`.

Copy every file of `input` into `out` under a normalized name: lowercase it, turn every run of characters other than `a`-`z`, `0`-`9` and `.` into a single `-`, then remove `-` at the start and end. Contents are copied unchanged. Handle the files in sorted order of their original names (byte order); when a normalized name was already taken by an earlier file, skip the file.

Print one line per file in that order: `ORIGINAL -> NEW`, or `ORIGINAL skipped` for a skipped one.

Grants: `input: Dir`, `out: Dir`

Examples: `Q3 Report_FINAL.TXT` becomes `q3-report-final.txt`; `__init__.py` becomes `-init-.py` and then `init-.py`.
