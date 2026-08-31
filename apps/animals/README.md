# animals — Tier 1 at its interesting end

The guess-the-animal game that shipped with every BASIC and Unix system worth using. It
knows nothing at first and learns one animal per round, forever.

It is here because it forces the framework to prove things `hello` does not.

## What it demonstrates

| Feature | Where |
|---|---|
| **Multi-event atomic writes** | `app.lua` → `pv.batch` emits three events as one |
| **`tx.append` returning minted ULIDs** | The new leaves are referenced before they exist |
| **Recursive SQL over event-sourced rows** | `lib/tree.lua` → `knowledge()` |
| **Stored session state** | The `cursor` table — a round survives a reload and continues on another device |
| **Splitting logic out of routes** | `lib/tree.lua` is `require`d by name |
| **Tombstones** | `reset` empties the tree; the log keeps every round you played |
| **Accessible forms** | `fieldset`/`legend` for the radio pair, labels on every input |
| **The HTMX / Alpine boundary** | Both on one screen — see below |

## Why this app exists, in one sentence

**The animal tree *is* the event log.** The system learns by appending, and nothing is ever
updated. Teach it "ostrich" and that is a new line in a JSONL file you can `cat`. That is
the founding invariant made visible to a non-technical person in about fifteen seconds —
`hello` cannot show it without ceasing to be minimal, and `sketch` never touches SQL.

## HTMX and Alpine, on one screen

These are often described as opposite philosophies. They are better understood as
complementary halves of one rule:

> **If losing it on refresh loses data, it is HTMX.
> If losing it on refresh is fine, it is Alpine.**

`animals` has both, unforced:

| Interaction | Tool | Why |
|---|---|---|
| Submit a guess | HTMX | appends an event |
| Teach a new animal | HTMX | appends events, atomically |
| Tree fragment re-renders | HTMX | the node owns the truth |
| "Show the question path" disclosure | Alpine | pure UI, nothing persisted |
| Play / History tab switch | Alpine | no round trip, no state worth keeping |
| Confirm before teaching | Alpine | ephemeral by definition |

Each use is commented in the source with *why that tool*, not *how it works*. The teaching
happens in the contrast, on one page.

Across the three reference apps a reader sees all three postures without a comparison
document: server-owned state (`animals`, HTMX), client-owned ephemeral state (`animals`,
Alpine), and client-owned everything (`sketch`).

## The learning step

You land on a leaf. The app guesses "is it a *penguin*?" You say no, and tell it you meant a
*wombat*, distinguished by "does it have cubic droppings?"

The naive move creates a new question node and re-points the parent at it — but the parent
may not exist (this could be the root), and finding it means a traversal.

Instead, **the leaf becomes the question, in place, keeping its own ULID:**

```
before:                  after:
                                [does it have cubic droppings?]
   [penguin]      →             /                            \
                           yes /                              \ no
                       [wombat]                            [penguin]
```

Three `put` events: one rewriting the leaf into a branch, two creating fresh leaves. The
parent is never touched, and every existing pointer into the tree stays correct because the
branch reuses the leaf's id.

That is only clean when identity is a ULID you control and writes are appends.

## Reading your own game history

```bash
duckdb -c "
  SELECT ts, op, d->>'text' AS text
  FROM read_json_auto('data/animals/log/*.jsonl')
  ORDER BY lam"
```

Every animal you ever taught it, in order, including the ones you reset away.

## Later: the sync demo

Once `spec/protocol.md §10` lands (roadmap Phase 3), this app becomes the best
demonstration of sync in the repository. Wire the history fragment to `/api/stream` with the
HTMX SSE extension — one attribute — and teaching an animal on the desktop updates the
phone's history live, on screen, with no polling code and no reload. Two devices, one
visible cause and effect.

---

Copyright © 2026 Gabriel Mongefranco
