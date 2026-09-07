<!--
Project:  Privatium™
File:     docs/sample-app-design.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-09-06
Modified: 2026-09-06
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
- **Sketch:** a separate back link and title sit above a compact drawing toolbar and framed
  white canvas. Bootstrap icons accompany the visible tool labels. Draw and Stroke eraser
  show their selected mode with text styling and an accessible pressed state. Labelled
  color presets and the picker live in a native Colors disclosure. Draw, Eraser, Stroke eraser and Undo stay on the toolbar. Help contains instructions;
  Sketch actions offers Download PNG and New sketch. A `ResizeObserver` matches the canvas backing store
  to its CSS size when tools or help change height. Keyboard drawing remains available;
  one finger draws, while the canvas permits pinch zoom. Stored points still use pixel
  coordinates: a smaller canvas can clip an older, larger drawing without deleting it.

Sketch offers a native color picker and a labelled hex field alongside its preset
inks. Stroke eraser removes the topmost stroke at the chosen point using an append-only
tombstone; Space or Enter performs the same action at the keyboard pen. The Apps link
returns to the launcher (Settings when run solo).

Sketch keeps its white drawing surface in either system theme;
changing the canvas background would change the appearance of saved strokes.

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

Starting a stroke closes open color and action panels. Eraser paints a 24-pixel white
stroke on the white canvas; Stroke eraser removes a whole stroke. Both work by keyboard.
Undo remembers up to 50 local actions in the current tab, including New sketch. Reloading
clears that undo history. A restoration uses fresh IDs and retains the original layer
key, because a tombstoned ID cannot be reused. Later changes to a touched stroke can
prevent undo; the page explains this instead of overwriting them.

New sketch clears the shared canvas with tombstones, rather than creating a sketch
library. Download PNG first to keep a separate image. PNG export includes saved strokes,
an opaque white background and no keyboard crosshair, up to 16 million pixels.

A correct Animals guess shows “I guessed it!” with a Start over button. The win page
is a presentation state, available with or without HTMX; it writes no event. Starting
over moves the cursor to the root and keeps the learned animals. The same restart
control sits beside What I know during play.

Sketch groups its icon-only Apps link and hamburger actions menu in the header.
Both retain accessible names and 44-pixel targets, using supplied Bootstrap Icons.

Shell availability notices use end-user language such as “coming soon”, without
phase numbers or implementation milestones. Technical detail stays in the documentation.

The shell footer links to the repository on the left and displays this space’s name
(or its ID when unnamed) on the right. The QR icon is a placeholder linking to Space
settings until the connection interface is available.

UI wording calls a node a “space” and uses “connect” for the connection action.
Protocol names, stored fields and routes keep their existing technical names.

---

Copyright © 2026 Gabriel Mongefranco
