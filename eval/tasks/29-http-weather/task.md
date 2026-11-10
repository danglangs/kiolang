# Weather station summary

`main` receives `api: Http`, granted GET access to a weather JSON API.

- `GET /stations` returns `{"stations": [{"id": "s1", "name": "North Ridge"}, ..]}`.
- `GET /stations/ID/readings` returns `{"station": "ID", "readings": [{"t": "2026-11-09T10:00Z", "temp": 123}, ..]}`, where `temp` is an integer in tenths of a degree (123 is 12.3, -5 is -0.5). It can fail with a non-2xx status.

Print one line per station, ordered by name (byte order):

- `NAME min=MIN max=MAX avg=AVG n=COUNT`, temperatures written in degrees with one decimal (`12.3`, `-0.5`, `0.0`), where `AVG` is the sum of the tenths divided by the count, truncated toward zero, in tenths;
- `NAME no data` when it has no readings;
- `NAME unavailable` when its readings request fails.

Grants: `api: Http`

Example: readings 10, -5 and 4 give `min=-0.5 max=1.0 avg=0.3 n=3`.
