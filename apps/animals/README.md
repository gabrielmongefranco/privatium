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

---

Copyright © 2026 Gabriel Mongefranco
