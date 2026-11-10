# Latency percentiles per endpoint

`main` receives `input: Dir`, granted for reading. `input/timings.log` has one request per line: `METHOD PATH MILLIS`, for example `GET /api/users 42`. Lines with a different number of fields or a non-integer `MILLIS` are ignored.

Group by `METHOD PATH`. For each group, sorted by method then path (byte order), print

```
METHOD PATH count=N p50=A p95=B max=C
```

where the percentiles use the nearest-rank method: with the group's times sorted ascending as `t[1..N]`, `pP` is `t[ceil(P * N / 100)]`.

Grants: `input: Dir`

Example: for

```
GET /a 10
GET /a 30
GET /a 20
POST /a 5
```

print

```
GET /a count=3 p50=20 p95=30 max=30
POST /a count=1 p50=5 p95=5 max=5
```
