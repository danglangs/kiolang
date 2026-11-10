# Cheapest route

`main` receives `input: Dir`, granted for reading. `input/routes.txt` describes a road network and the trips to plan:

```
road A B 7
road B C 2
trip A C
```

`road X Y COST` is a two-way road between towns `X` and `Y` with a positive integer cost. `trip X Y` asks for the cheapest route from `X` to `Y`. Lines may come in any order; blank lines and lines starting with `#` are ignored.

For each trip, in input order, print `X -> .. -> Y COST`, the towns along a cheapest route and its total cost. When several routes tie, print the one whose list of towns is smallest compared town by town (byte order of names). When `Y` cannot be reached print `X Y unreachable`. A trip from a town to itself is `X 0`.

Grants: `input: Dir`

Example: for the three lines above print `A -> B -> C 9`.
