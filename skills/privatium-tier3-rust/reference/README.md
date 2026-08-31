# Generated reference

The reference files this skill cites are **generated from the crate and the specification at
build time** and are not committed. Until the implementation exists, this directory is empty.

`docs/skills.md §7` makes the rule explicit: a change to `spec/` that is not reflected in
`skills/` is an incomplete change, and CI fails on drift between the generated reference and
its source.

Until then, read the specs directly — they are the contract, and the generated files will be
a restatement of them.
