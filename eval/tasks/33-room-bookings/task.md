# Room bookings: busy, free and conflicts

`main` receives `input: Dir`, granted for reading.

`input/bookings.csv` starts with the header `room,day,start,end,who`; each other line is one booking, such as `blue,2026-11-10,09:30,10:30,bob`. Times are `HH:MM` with exactly two digits each, hours 00-23 and minutes 00-59; `end` may also be `24:00`. A line is skipped if it does not have exactly five fields, a time is malformed, or `end` is not after `start`. Blank lines are ignored. A booking that repeats an earlier one exactly (all five fields) is ignored.

For each room (sorted by name, byte order) and, within it, each day that has bookings (sorted), print:

```
ROOM DAY
  busy 09:00-11:00, 13:00-13:20
  free 11:00-13:00, 14:00-18:00
  conflict ann/bob 09:30-10:00
```

- `busy`: the union of the room's bookings that day, as merged intervals in time order; intervals that touch (`10:00-11:00` and `11:00-12:00`) merge.
- `free`: the gaps of at least 30 minutes between 09:00 and 18:00 not covered by any booking, in time order; `free none` if there are none.
- `conflict`: one line for every pair of bookings that overlap by at least one minute (touching is not overlapping), with the two `who` names sorted (byte order) and joined by `/`, then the overlapping interval. Lines sorted by the overlap's start, then by the line's text. No line when there is no conflict.

Last, print `skipped: N1, N2, ..`, the line numbers of skipped lines (the header is line 1), or `skipped: none`.

Grants: `input: Dir`
