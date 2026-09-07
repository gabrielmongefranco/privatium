<!--
Project:  Privatium™
File:     docs/sample-app-design.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-06
Modified: 2026-09-07
Summary:  How the reference apps keep their presentation small and responsive.
          See main README.md for full license information.
-->

# Sample app design

The reference apps use system fonts, ordinary HTML and small external stylesheets.
You do not need a build step or a new library to change their appearance.

## Shared styles

The shell uses neutral white and slate surfaces with dark text and blue actions.
The supplied wordmark remains in the header; the website's lilac and ivory palette
is not used for application surfaces. A hamburger button, labelled Menu for assistive technology, uses a native
disclosure to expose all shell
pages. Every shell header uses the same compact padding and 44-pixel navigation targets.
Apps remains a shortcut; Settings is available in the menu.

Hello and Animals inherit the shell's colors, form controls and focus rings. Their
own styles use those color tokens, so they follow the shell's light and dark modes.
The launcher displays each loaded app's manifest description, or its slug when the
description is missing or blank. Descriptions are escaped text.

Settings uses the same palette, with a wrapping tab navigation and labelled groups of
metadata. Device and alert tables retain their icons and columns on larger screens;
below 40rem each row stacks its labelled values. Table roles and column headers remain
available to assistive technology, while repeated visual labels are hidden from it.

Navigation wraps, the launcher becomes a single column, and controls keep a minimum
height of 44 CSS pixels. Long text wraps instead of widening the page. At narrow
widths, reduce padding and stack controls; do not shrink text to make it fit.

## Each app's layout

- **Hello:** a greeting panel and one short form. `static/hello.css` adds presentation
  without adding application logic. The name field receives focus when its form opens.
- **Animals:** a prominent question board, large answer buttons and a table of learned
  animals. Expanded question paths wrap below their buttons. HTMX still submits the
  game forms; the same forms work without JavaScript on loopback. Existing Alpine CSP
  components still control disclosures and reset confirmation. After an HTMX swap,
  focus moves to the new question so Tab reaches its answers.
- **Sketch:** a labelled tool rail down the left, a single-row top bar, the white sheet
  centred between them, and a status footer. The rail lists all ten tools one per line as
  an icon, a name and its keyboard letter, with the active tool's own options directly
  beneath. The top bar carries the tools reached most often, and then whatever the current
  tool needs — the colour row for the tools that lay ink down, one line of explanation for
  the ones that do not — with undo, redo, the high-contrast toggle and the Sketch actions
  menu always on its right. Zoom sits in the sheet's own bottom-right corner. Below
  820 pixels the rail becomes a strip under the sheet and the tools scroll sideways;
  nothing drops below a 44-pixel target. Every icon is a vendored Bootstrap Icon inlined
  into the page's sprite. Keyboard drawing remains available; one finger draws, while the
  sheet permits pinch zoom.

The sheet is a fixed 1600 by 1200 coordinate space, sized in CSS and drawn through a
context transform, so a mark lands on the same pixels on every device that shares it. The
canvas's backing store still follows its CSS box at the device pixel ratio. Zoom is
per-device view state kept in `localStorage`, never in the log.

Sketch keeps its white drawing surface in either system theme;
changing the sheet background would change the appearance of saved strokes. Its own
`--muted` token draws the boundary of every control, because the hairline `--line` used
between regions is 1.38:1 against the panel and does not meet WCAG 1.4.11 on its own.
Colour swatches compute their real contrast against the current panel and take a ring
when their fill does not clear 3:1 by itself. The Apps link returns to the launcher
(Settings when run solo).

## Check a change

Run `privatium lint apps/hello apps/animals apps/sketch`. The repository's
`cargo test --locked -p privatium-core --test reference` also exercises the apps and
checks rendered accessibility structure and declared color contrast.

Then use a browser at desktop, 320-pixel and 200-pixel widths. Check keyboard operation,
visible focus, 200% zoom, long names, expanded paths, errors and Sketch's help. Complete
the main flows with a screen reader and test touch on a real phone. A 200-pixel browser
check is a layout stress test, not a claim of smartwatch compatibility.

App templates, scripts and styles can be refreshed without compiling Rust. Shared
shell styles are embedded in the executable and require a rebuild.

## TODO: customizable shell

Planned follow-up, not implemented: load an optional
`<data-root>/privatium-shell.css` after the built-in shell stylesheet. Refreshing the
page should pick up edits without rebuilding Rust. Keep this file outside the
append-only `data/` logs. The implementation needs a specified asset route, cache
behaviour and the encrypted channel's stylesheet integrity handling; it must preserve
the default Content Security Policy. This belongs in the specification before code.

An alternative is a bundled system app named `privatium`, containing the default
presentation assets and an owner-editable copy that overrides them. Keep authorization,
framework routes and settings operations in the core, with embedded assets as fallback.
Before implementing this alternative, specify its reserved name, loading precedence,
asset boundary and behaviour when files are missing or invalid. Neither approach is
implemented by the sample app refresh.

## Sketch editing and export

Starting a mark closes open colour and action panels. The eraser paints white ink at the
chosen width; Select picks whole marks and moves them, and Delete removes them. All of it
works by keyboard. Undo reverses the latest change to the shared canvas, whichever window
or device made it — a mark, an erasure, a move, a deletion or a New sketch — up to 50
changes back; the log replayed at load fills that history, so a reload loses nothing
undoable. A batch written as one act is undone as one. A restoration uses fresh IDs and
retains the original layer key, because a tombstoned ID cannot be reused. Later changes
to a touched mark can prevent undo; the page explains this instead of overwriting them.

An action that would name more events than the node accepts in one batch is written in
ceiling-sized chunks instead of being refused whole, and the status line says how many
undo presses it now takes.

New sketch clears the shared canvas with tombstones, rather than creating a sketch
library. Download PNG first to keep a separate image. PNG export renders the sheet at
1600 by 1200 whatever the zoom, with an opaque white background and no keyboard
crosshair. Download SVG writes the marks as shapes; a flood fill is pixels rather than a
shape, so it is left out and the count is reported.

A correct Animals guess shows “I guessed it!” with a Start over button. The win page
is a presentation state, available with or without HTMX; it writes no event. Starting
over moves the cursor to the root and keeps the learned animals. The same restart
control sits beside What I know during play.

Sketch groups its Apps link and the rest of its actions in one menu on the top bar's
right. Every icon-only control keeps an accessible name and a 44-pixel target, using
supplied Bootstrap Icons.

Shell availability notices use end-user language such as “coming soon”, without
phase numbers or implementation milestones. Technical detail stays in the documentation.

The shell footer links to the repository on the left and displays this space’s name
(or its ID when unnamed) on the right. The QR icon is a placeholder linking to Space
settings until the connection interface is available.

UI wording calls a node a “space” and uses “connect” for the connection action.
Protocol names, stored fields and routes keep their existing technical names.

---

Copyright © 2026 Gabriel Mongefranco
