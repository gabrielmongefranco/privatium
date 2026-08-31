---
name: privatium-app-animals
description: Context for extending the animals reference app — its decision-tree schema, the three-event learning step, and its recursive SQL. Load alongside privatium-tier1-lua when modifying this specific app.
---

# animals

Tier 1. A binary decision tree that learns one animal per round.

## Schema

- `node(id, kind 'q'|'a', text, yes_id, no_id)` — leaves are animals, branches are questions
- `cursor(id='cursor', node_id, started)` — single row, where the current round is

## The learning step — do not "simplify" it

`POST /teach` emits exactly three events in one `pv.batch`:

1. The leaf you landed on is **rewritten in place into a question**, keeping its own ULID
2. A new leaf for the new animal
3. A new leaf carrying the old animal's text

The parent is never touched and never has to be found. Because the branch reuses the leaf's
id, every existing pointer into the tree stays correct. A version that creates a new node
and re-points the parent is wrong: the parent may not exist (the leaf may be the root), and
finding it requires a traversal.

## `lib/tree.lua`

Queries live here, not in `app.lua`. `root_id()` finds the node nobody points at.
`knowledge()` is a recursive CTE over event-sourced rows. `clean()` trims and rejects empty
input.

## Conventions to preserve

- `tx.append` returns the minted ULID; the branch references both new leaves before they
  exist. Do not pre-generate ids outside the batch.
- `reset` writes tombstones. It never rewrites a log.
- The radio pair in `views/teach.lsp` is wrapped in `fieldset`/`legend`. Keep it.

Run `privatium lint apps/animals` before finishing.
