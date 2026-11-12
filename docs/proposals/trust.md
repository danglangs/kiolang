# Trust: what a reviewer approves instead of the code

kio is a language for generated software whose important guarantees can be checked independently of the agent that wrote it. People approve intent, authority and evidence; machines handle the implementation.

**Independently** is the test for every feature: a guarantee comes from the checker, the host, a verifier or a reproducible run, never from another agent's judgement alone. Capabilities are the model:

```
person decides     this program may read orders/ and POST to the CRM
language contract  authority is passed explicitly (main's parameters)
independent check  the checker derives each operation; the host enforces each grant
```

## The reviewer's questions

| Question | Made explicit by | Status | Task |
|---|---|---|---|
| Where can it act? | capabilities, the authority report and diff | done | P1-P7 |
| What did it do? | the audit log, replay | done | H-tasks |
| What may it consume? | enforced limits; budgets against a base | limits done | PG1, R1 |
| Is it right on these inputs? | `#[test]`, `assert` | building | T1 |
| Is it right on any input? | properties with shrinking; contracts on functions | pinned | T2, C1 |
| Can it happen twice, or out of order? | contracts first; typestate or linear tokens if F1 keeps needing them | open | F1, C1 |
| Can sensitive data reach that host? | information-flow labels, explicit declassification | research | |
| Is what ran what was reviewed? | hashes of source, compiler and grants, checked at run | research | R1 |

## Assurance levels

Every line of evidence says which it is: **proved** (the checker, for all inputs, under stated assumptions), **enforced** (the host stops a violation when it happens), **tested** (held on the inputs that ran), **measured** (these runs). R1's review page labels each line.

## The forcing function

F1 (a durable workflow service) supplies the concrete questions: an amount never negative, an event never applied twice, an acknowledgement never before durable acceptance, customer details never in an outgoing notification. Each becomes a type, a contract or a property, and the ones that need new language ideas become proposals.
