# Season leaderboard

`main` receives `input: Dir`, granted for reading, and `board: Kv`, a key-value store that keeps its contents between runs (empty on the first run).

`input/games.txt` has one finished game per line: `WINNER LOSER`. The winner gains 3 points, the loser 0, and both are counted as having played one game. A line `WINNER LOSER draw` is a draw: each gains 1 point. Player names have no spaces. Other lines are ignored.

Totals accumulate over runs. After applying the file, print the top 3 players of the season as `RANK. NAME POINTS PLAYED`, ordered by points, highest first, then by fewer games played, then by name (byte order). Ranks are 1, 2, 3 even on ties. Print fewer lines when fewer players exist.

Grants: `input: Dir`, `board: Kv`
