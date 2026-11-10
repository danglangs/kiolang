# Build stages, cycles and missing dependencies

`main` receives `input: Dir`, granted for reading.

`input/packages.json` maps each package name to the list of packages it depends on: `{"app": ["web", "log"], "web": ["log"], "log": []}`. An entry whose value is not an array of strings is invalid: it is not a package, so a dependency on it is a missing dependency. A name listed twice in one array counts once.

A package can be built once all its dependencies are built. Its stage is 1 if it has no dependencies, else 1 + the largest stage among its dependencies. A package is not built if it depends on a missing package, if it is part of a dependency cycle (a package that depends on itself, directly or through others), or if it depends on a package that is not built.

Print, in this order and omitting any section that is empty:

- `stage N: P1, P2, ..` for each stage from 1 up, the packages of that stage sorted by name (byte order);
- `cycle: P1, P2, ..` for each group of packages that all depend on each other (a strongly connected component with two or more packages, or one package that lists itself), the group sorted, groups sorted by their first name;
- `missing: M (needed by P1, P2, ..)` for each missing name, sorted, with the packages that list it, sorted;
- `invalid: N1, N2, ..`, the invalid entries, sorted;
- `blocked: P1, P2, ..`: the packages not built that are in no cycle, sorted;

and last, always, `built B of N`, where `N` counts the valid packages.

If the file is not a JSON object, print only `error: not an object`.

Grants: `input: Dir`
