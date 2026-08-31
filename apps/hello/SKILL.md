---
name: privatium-app-hello
description: Context for extending the hello reference app — its schema, routes, and conventions. Load alongside privatium-tier1-lua when modifying this specific app.
---

# hello

Tier 1. Stores one name and greets you.

## Schema

`profile(id VARCHAR PK, display_name VARCHAR NOT NULL)` — at most one row, ever.

## Routes

| Route | Handler |
|---|---|
| `GET /` | Greeting, or an invitation if no profile exists |
| `GET /edit` | The form |
| `POST /name` | Trims, validates, appends |

## Conventions to preserve

- `pv.append('profile', me.id, ...)` **reuses the existing id.** That is what makes an edit
  an amendment rather than a second person. Do not mint a new ULID on save.
- Every link goes through `url()`. This app works unmodified in solo mode.
- Templates use `<?= ?>` only. There is no `<?raw ?>` here and there should not be.

## Extending it

Adding a field means one column in `schema.sql`, one input in `views/edit.lsp`, and one key
in the `pv.append` call. The schema change rematerializes from the logs automatically —
existing events simply lack the key and the column is NULL for them.

Run `privatium lint apps/hello` before finishing.
