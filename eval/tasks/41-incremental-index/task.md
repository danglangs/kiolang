# An incremental search index

`main` receives `docs: Dir` and `queries: Dir`, both granted for reading, and `index: Kv`, a key-value store that keeps its contents between runs (empty on the first run).

`docs` holds text files, in subdirectories at any depth. A file's path is relative to `docs`, with `/` between directories (`sub/deep/d.txt`). A file's words are the maximal runs of bytes that are not ASCII characters other than letters and digits (so `fox-trot` is `fox` and `trot`; `crème;` is `crème`), lowercased (`to_lowercase`: `CAFÉ` is `café`).

Each run, compare the files with those of the previous run and print, sorted by path (byte order):

- `added PATH` for a file that is new, `changed PATH` for one whose contents differ, `removed PATH` for one that is gone;

then `unchanged N`, the number of files whose contents are the same. Keep in `index` what the next run needs, and an index from word to files that stays correct as files change. Unchanged files need not be read again (you may compare a hash).

Then answer each non-blank line of `queries/queries.txt`: its words (split and lowercased the same way) must all appear in a file. Print `LINE: P1, P2, ..` (the line as written, then the matching paths, sorted), or `LINE: none`.

Grants: `docs: Dir`, `queries: Dir`, `index: Kv`
