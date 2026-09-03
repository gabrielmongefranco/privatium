# Vendored: htmx

| | |
|---|---|
| Project | [bigskysoftware/htmx](https://github.com/bigskysoftware/htmx) |
| Version | **v2.0.9** |
| File | `htmx.min.js`, the release asset of that tag, unmodified |
| SHA-256 | `57d9191515339922bd1356d7b2d80b1ee3b29f1b3a2c65a078bb8b2e8fd9ae5f` |
| Licence | Zero-Clause BSD (0BSD) — reproduced in the repository `NOTICE` |
| Vendored | 2026-09-03 |

`htmx.min.js` is the only third-party file here; `shell.css` is the framework's own and
carries the repository header. htmx is served at `/static/htmx.min.js` and loaded by the
shell under the default Content-Security-Policy of `spec/protocol.md §9.3` — no inline
script, and the shell's `htmx-config` turns `allowEval` and `allowScriptTags` off, so the
few htmx features that need `eval` are never reached (`AGENTS.md`).

Not htmx 4: it was released while Phase 1 was underway and 2.x is the line the
documentation and the reference apps were written against.
