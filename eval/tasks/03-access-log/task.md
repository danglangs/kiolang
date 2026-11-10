# Access log summary

`main` receives `input: Dir`, granted for reading. `input/access.log` is a web server log in Common Log Format, one request per line:

```
203.0.113.9 - - [09/Nov/2026:10:00:01 +0000] "GET /index.html HTTP/1.1" 200 2326
```

The fields after the quoted request are the status code and the response size in bytes (`-` means 0).

Print:

1. one line per status code, `STATUS COUNT`, by status code ascending;
2. `bytes TOTAL`, the sum of the sizes;
3. `top PATH COUNT`, the path requested most often (the request's second word, without any `?query`); equal counts: the alphabetically first path;
4. `errors IP COUNT` for each client IP with at least one 5xx response, by count highest first, then IP alphabetically.

Lines that do not have this shape are ignored.

Grants: `input: Dir`

Example: for

```
10.0.0.1 - - [09/Nov/2026:10:00:01 +0000] "GET /a?x=1 HTTP/1.1" 200 10
10.0.0.2 - - [09/Nov/2026:10:00:02 +0000] "GET /a HTTP/1.1" 500 -
```

print

```
200 1
500 1
bytes 10
top /a 2
errors 10.0.0.2 1
```
