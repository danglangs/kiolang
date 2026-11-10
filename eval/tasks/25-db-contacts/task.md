# Contact sync into SQLite

`main` receives `input: Dir`, granted for reading, and `crm: Db`, a SQLite database that keeps its contents between runs (empty on the first run; create your schema if it is missing).

`input/contacts.csv` is a daily export with a header line and rows `email,name,phone`. A contact is identified by its email, trimmed and lowercased. For each row in order:

- a new email is inserted;
- a known email whose name or phone (trimmed) differs from the stored one is updated to the new values;
- otherwise it is unchanged.

A row without exactly three fields or with an email lacking `@` is rejected. Blank lines are ignored. Later rows of the same file see the effect of earlier ones.

Print `inserted I updated U unchanged N rejected R`, then `contacts TOTAL` with the number of stored contacts, then the stored contacts whose phone is empty, as `missing phone: EMAIL`, sorted by email.

Grants: `input: Dir`, `crm: Db`
