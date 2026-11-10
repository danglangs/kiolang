# A persistent LRU cache

`main` receives `input: Dir`, granted for reading, and `cache: Kv`, a key-value store that keeps its contents between runs (empty on the first run).

Each run replays `input/ops.txt` against an LRU cache whose contents and statistics live in `cache` across runs. The file's first line is `capacity N` (an integer, at least 1). Every other line is one operation, its words separated by single spaces:

- `get KEY`: print `KEY=VALUE` if the key is cached (a hit; the key becomes the most recently used), else `KEY miss` (a miss).
- `put KEY VALUE`: cache the value under the key, replacing any old value; the key becomes the most recently used. `VALUE` is the rest of the line after `put KEY ` and may contain spaces and any Unicode. If the key is new and the cache already holds `capacity` keys, first evict the least recently used key and print `evict KEY`.
- `del KEY`: remove the key if cached; print nothing.

A blank line is ignored. Any other line (an unknown operation, a missing or extra word, an empty `KEY` or `VALUE`) prints `bad line N`, where `N` is its line number in the file (the capacity line is line 1), and changes nothing.

The capacity can differ from the previous run's. If the cache holds more keys than the new capacity, evict least recently used keys until it fits, before the first operation, printing `evict KEY` for each.

After the operations, print `cache: K1,K2,..` (the cached keys, most recently used first; `cache:` alone when empty), then `stats hits H misses M evictions E`, the totals over all runs so far.

Grants: `input: Dir`, `cache: Kv`
