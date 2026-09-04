# apps/_lint/

The linter's corpus (`spec/cli.md §5.4`, `docs/roadmap.md` Phase 1): for every rule of
`spec/cli.md §5.1`, one app that passes it and one that fails it, under
`pass/<rule>/<slug>/` and `fail/<rule>/<slug>/`. The rule directory holds the app rather
than being it, because `PV104` compares the slug to the folder name and `PV104` is not a
slug.

Rules:

- A `pass` app is clean under every rule, not only its own.
- A `fail` app trips its rule; what else it trips is incidental, kept to a minimum, and
  never the reason the fixture exists.
- A rule directory may hold a `config.toml`: the node configuration the fixture is linted
  under. `pass/PV502` needs solo mode. The test harness reads it; from the command line
  it is `privatium lint --config apps/_lint/pass/PV502/config.toml apps/_lint/pass/PV502`.
- `crates/privatium-core/tests/lint.rs` fails when a rule has no pair, and when a
  directory here is not named after a rule.

The loader never mounts anything under `_lint/`: a folder whose name starts with `_` is
not an app (`docs/plans/phase-1.md §2.6`).

Lint one by hand:

```bash
privatium lint apps/_lint/fail/PV301
privatium lint apps/_lint/fail/PV301 --format json
```

---

Copyright © 2026 Gabriel Mongefranco
