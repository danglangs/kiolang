# Packages (PK): local dependencies with pinned identities

Status: proposal, 2026-11-13. A one-way door: a file format programs and CI will depend on.

## The need

`mod web;` loads `web.kio` beside the root only. F1 (kio-web `examples/flow`) copies kio-web's `web.kio` and `api.kio` in before every `kio check`, `kio test` and `kio run`, and deletes them after; a run between the two fails "cannot read web.kio" (F1.1 friction 2, F1.2 friction 5). Every kio-web example does the same.

## The trust question

What can a reviewer decide without reading a dependency? Two things, both checked independently of whoever wrote it:

| Question | Answered by |
|---|---|
| What can the dependency do? | Nothing on its own: a library has no ambient authority, only the capabilities a caller passes (the checker). Its calls are charged to the caller's grants in the authority report, so `kio check --base` shows a dependency update that starts calling `db exec`. Already true today. |
| Is it the code that was reviewed? | Its sha256, pinned in a file the reviewer sees in the diff; `check`, `test`, `run` and `review` refuse a file that does not match (new). |

## Proposal

No language change: `mod web;` stays. A file `kio.deps` beside the root says where a module comes from and pins its content:

```
# kio.deps: one module per line; a path is relative to this file
web  ../../web.kio  sha256:e4aad9a99560d8fd3d5f893e372764cc315c1b08611af9efce55b17e0fa6959e
api  ../../api.kio  sha256:889e267b960dfd2fac6e7299d7c9942067276d5824e2961ca35b61a5c8902660
```

```
app.kio:  mod web;  mod api;  mod flow;
              |          |          |
          kio.deps   kio.deps   flow.kio beside app.kio (as today)
```

| Rule | |
|---|---|
| Resolution | A `mod NAME;` listed in `kio.deps` loads that file; one not listed loads `NAME.kio` beside the root, as today. A name both listed and present beside the root is an error ("`web` is in kio.deps and beside app.kio" hint "delete one"). |
| Pinning | Every line carries a hash. A mismatch stops `check`, `test`, `run`, `review` and `--listen` before compiling: "web.kio does not match kio.deps (sha256:e4aa.. pinned, sha256:91c0.. found)" hint "review the change, then `kio deps pin`". |
| `kio deps pin` | Rewrites the hashes in `kio.deps` from the files as they are; prints each line that changed. The only command that writes the file. |
| Flat | Modules stay flat: a dependency file may use other declared modules (`web` uses `http`), so the root's `kio.deps` lists every file the program loads. No transitive resolution in v1. |
| Visibility | `kio check` names each dependency and its hash; `kio review` lists dependencies added, removed or changed (old and new hash, `not checked`: "a dependency's code changed; its authority shows above"). The audit log's `start` line includes the hashes. |
| Where files live | Local paths only: a sibling checkout, a git submodule or a vendored copy. kio never fetches; `check` and `run` read only the listed files. |

## Not now

- **Fetching** (`web  git https://github.com/danglangs/kio-web 4f2a91c  sha256:..`): `kio deps fetch` into a cache, never during `check` or `run`. Added when a second team publishes a library; the line format leaves room for it.
- **A registry, versions, semver ranges**: an exact hash per file is what review needs; ranges are what review cannot approve.
- **Packages of several files with private modules**, nested namespaces: once a library needs internal structure.

## Options considered

| | Language change | Identity pinned | Verdict |
|---|---|---|---|
| A. `kio.deps` with hashes (above) | none | yes, in the reviewed diff | recommended |
| B. A path in source: `mod web = "../../web.kio";` | yes, a new form | no; a second mechanism needed for hashes | no |
| C. A `--mod web=PATH` flag (and in the grants file) | none | no | no: dependencies are code, not grants; `check` and `test` would need the flag too |
| D. A full manifest (name, version, authors, ranges, registry) | none | partly | later, if ever; most of it does not help a reviewer |

## Done (when approved)

kio-web's examples drop their `cp` lines and ship a `kio.deps`; `kio check` reports the hashes; a mismatch and a name in both places are errors with hints; `kio deps pin`; `kio review` shows a dependency change; guide.md section; tests for each.
