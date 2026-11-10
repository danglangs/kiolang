# Sessions from a click log

`main` receives `input: Dir`, granted for reading.

`input/events.log` has one event per line: `TIME USER PAGE`, separated by single spaces, where `TIME` is `YYYY-MM-DDTHH:MM:SSZ` (UTC; every part exactly that many digits; a real date, leap years included; hours 00-23, minutes and seconds 00-59). Lines are in no particular order. A line with another number of fields or an invalid time is malformed. A line that repeats an earlier line exactly is a duplicate and ignored. Blank lines are ignored.

Group each user's events into sessions: sorted by time, a new session starts when an event comes more than 30 minutes (1800 seconds) after the previous event of the same user. A session's duration is its last event's time minus its first's, in seconds.

Print, for each user sorted by name (byte order):

`USER sessions S events E longest HH:MM:SS pages P`

where `E` counts the user's events (duplicates excluded), `longest` is the longest session's duration (each part two digits; hours more if needed), and `P` counts the user's distinct pages. Then:

- `median D` the median session duration in seconds over all sessions of all users (for an even count, the lower of the two middle values);
- `top: PAGE (N), PAGE (N), PAGE (N)`, the 3 pages with the most events over all users (fewer if there are fewer pages), most first, equal counts by page (byte order);
- `malformed M duplicates D`.

With no valid events, print only the last line.

Grants: `input: Dir`
