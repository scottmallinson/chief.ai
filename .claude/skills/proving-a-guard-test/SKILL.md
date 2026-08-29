---
name: proving-a-guard-test
description: Use when writing or reviewing a test that guards an invariant rather than exercising a feature — a cap, a boundary, a "never overwrites", a "no network call", a "nothing was written". These pass on the day they are written whatever they assert, so this is how to find out whether they assert anything.
---

# Proving a guard test

A feature test fails before the feature exists. **A guard test passes immediately**,
because the thing it forbids has not happened yet. That is the whole problem: it
gives the same green tick whether it is watching the invariant or watching nothing
at all, and nobody finds out until the day it was supposed to fail and didn't.

Everything below was learned by writing one of these, believing it, and being wrong.

## The rule

**A guard test is not finished until you have watched it fail.**

Break the thing it guards, run it, read the failure message, put the thing back.
It costs two minutes and it is the only evidence you will ever get.

```bash
# 1. write the guard test, watch it pass
# 2. break the guard — the smallest edit that violates the invariant
# 3. run ONLY that test; it must fail, and the message must name the real cause
# 4. restore, and confirm green again
```

Restore from a copy, not from memory:

```bash
cp src/thing.rs /tmp/thing.bak     # before breaking
cp /tmp/thing.bak src/thing.rs     # after
```

## Three ways a guard test lies

### 1. It passes because something else is doing the work

The worst kind, because the invariant *is* held — just not by the code under test,
and not by anything the test names.

A test asserted a profile file the user had edited was never overwritten. Disabling
the overwrite guard left it green. A second guard, an early return further up,
was quietly holding the line; the test had never exercised the one it was named
after. Only disabling **both** made it fail.

> **When breaking one guard leaves the test green, you have found a second guard,
> not a robust system.** Keep looking until it fails. Then decide, deliberately,
> whether the redundancy is defence in depth worth keeping — and say so in a
> comment, because the next person will find one of them and think it is dead code.

### 2. It passes vacuously

A loop over an empty collection asserts nothing and reports success.

```rust
// Green when the stub was never called at all.
for request in server.await.expect("server") {
    assert!(request.starts_with("GET "), "must not write anything");
}
```

The fix is to assert the collection is non-empty *first*, and to say why:

```rust
let requests = server.await.expect("server");

assert!(
    !requests.is_empty(),
    "the pass should have read GitHub; an empty list proves nothing"
);
```

The same shape hides in `.iter().all(...)`, `.find(...).is_none()`, and any
assertion that a thing is *absent* from a set nobody filled.

### 3. It passes because the boundary is nowhere near it

A cap of "6 tools and 600 tokens" against a catalogue of 3 tools and 477 tokens is
true, and stays true through changes that were supposed to trip it. Breaching it
proved which half binds: a seventh tool trips the count, but the *token* ceiling is
reached at the fourth. Only one of the two numbers is doing any work, and the test
comment should say which.

**Breach every clause separately.** A test with two assertions needs two breaches,
or you have proved one of them.

## What counts as a guard test

If the assertion is about something *not* happening, it is one:

- caps and budgets — "no more than N", "under X tokens"
- boundaries — "no network call", "nothing outside this directory"
- preservation — "never overwrites", "the user's edit survives"
- atomicity — "a failure writes nothing", "no orphan row"
- absence — "this does not appear in the output", "the list is empty"

## Breaking it well

The breach should be the **smallest edit that violates the invariant**, and it
should look like a plausible future mistake rather than sabotage:

| Guard | Breach |
| ----- | ------ |
| A failure writes nothing | Move the write *before* the fallible call |
| Never overwrites the user's file | Replace the condition with `true` |
| No network call in this step | Add the write the next step will legitimately add |
| Under N of something | Add an N+1th, then fatten one until the other clause trips |

If you cannot think of an edit that breaks it, the test may be asserting something
that cannot vary — which is worth knowing before it is committed.

## Writing down what you found

The breach is evidence and it evaporates. Put the failure message in the pull
request:

```
the catalogue has 7 tools and D2 caps it at 6
the catalogue costs about 715 tokens and D2 caps it at 600
```

And put the *surprising* part in a comment beside the code, not only in the PR —
that a second guard exists, that only one clause binds, that the loop needed a
non-empty check. The PR is read once; the comment is read every time somebody
touches the test.

## Red flags

| Thought | Reality |
| ------- | ------- |
| "It passes, so the guard works" | It passed before you wrote the guard too. |
| "Breaking it deliberately is silly" | It is the only difference between a test and a comment. |
| "It's obviously correct by construction" | Then say which construction, in the comment. |
| "The type system already prevents this" | Then the test is documentation — fine, but label it. |
| "I'll trust it, the CI is green" | CI is green on a test that asserts nothing, too. |
