# Vendored: Bootstrap Icons

| | |
|---|---|
| Project | [twbs/icons](https://github.com/twbs/icons) |
| Version | **v1.13.1** (see `VERSION`) |
| Tag commit | `ce0e49dd063243118a115f17ad1fe1fe7576d552` |
| Released | 2025-05-09 |
| Licence | MIT — `LICENSE` beside this file, verbatim from the tag |
| Taken | the `icons/` directory of the tag, every `.svg` (2,078 files), and `LICENSE`; nothing else |
| Vendored | 2026-09-03 |

Every file in this directory except `VENDOR.md` and `VERSION` is third-party and carries
its own provenance; none gets the repository's header block (`AGENTS.md`, Style).

The set is vendored **in full**, not subset at build time: apps are installed at runtime
and declare their own icon in `app.toml`, so a build-time subset cannot know about them
(`docs/icons.md`). The framework embeds this directory in the binary with `include_dir!`
and inlines the requested icon into the HTML at render.

To move the pin: change `docs/icons.md`, replace the `.svg` files and `LICENSE` from the
new tag, update `VERSION` and this table, and run `cargo xtask icons-verify`, which fails
if any icon the shell, the reference apps, the skills or `docs/icons.md` name is missing.
