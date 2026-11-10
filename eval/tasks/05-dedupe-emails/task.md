# Deduplicate a mailing list

`main` receives `input: Dir`, granted for reading. `input/emails.txt` has one address per line, typed by people.

Normalize each line: trim spaces and lowercase it. It is valid when it contains exactly one `@`, with a non-empty part before it and a domain after it that contains a `.` and neither starts nor ends with `.`. For the domain `gmail.com`, also drop everything from the first `+` in the local part, and remove every `.` from the local part (`J.Doe+news@Gmail.com` becomes `jdoe@gmail.com`).

Print the distinct normalized valid addresses sorted (byte order), one per line, then `invalid N` where N counts the non-blank lines that were not valid. Blank lines are ignored.

Grants: `input: Dir`

Example: for

```
 Bob@Example.com
bob@example.com
J.Doe+news@Gmail.com
not-an-email
```

print

```
bob@example.com
jdoe@gmail.com
invalid 1
```
