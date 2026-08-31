---
name: privatium-accessibility
description: Accessibility requirements for every Privatium app, targeting WCAG 2.2 AA. Covers forms, icons, status messages, keyboard operation, and the two pairing paths that must both work. Load alongside whichever tier skill you are using — this applies to Lua, web, and Rust equally.
---

# Privatium Accessibility

Target: **WCAG 2.2 AA**. This applies to every tier. Load it alongside the tier skill, not
instead of it.

The framework serves people managing medications, chronic conditions, and mental health.
Assume some of them are tired, in pain, using a screen reader, or dyslexic — because some
of them are.

## MUST

**Forms**
- Every input has a `<label for>`. `placeholder` is not a label and disappears on focus.
- Group radios and checkboxes in `<fieldset>` with a `<legend>`
- Errors are text next to the field, announced with `role="alert"`, and never colour-only
- `autocomplete` on name, email, address, and telephone fields
- Do not disable zoom or set `maxlength` so tight it truncates real input

**Icons**
- Decorative icons beside text: `aria-hidden="true"` (the framework's default)
- Icon-only controls: `icon('trash', 'Delete this fill')` in Lua, or `aria-label` in HTML.
  **An icon-only button with no label is a bug and the linter fails it.**

**Colour and contrast**
- 4.5:1 for body text, 3:1 for large text and UI boundaries
- Never colour alone — pair it with text, an icon, or a pattern

**Keyboard**
- Everything reachable and operable by keyboard, in a sensible order
- Visible focus indicator; never `outline: none` without a replacement
- No keyboard trap. A modal returns focus where it came from.

**Structure**
- One `<h1>` per page, headings in order, no level skipped
- Landmarks: `<main>`, `<nav>`, `<footer>`
- Real `<table>` with `<th scope>` for tabular data — never a grid of divs

**Motion**
- Honour `prefers-reduced-motion`
- Nothing flashes more than three times per second

**Language and clarity**
- Set `lang` on the document
- Plain language. Short sentences. Say "due in 3 days," not "T-minus 72h."
- Never rely on the user remembering a screen they have left

## Pairing — both paths are required

The pairing flow MUST be completable **without reading text** (the 16-glyph emoji pad, with
labels) **and without seeing images** (the two-word code, read by a screen reader). Both.
Not either. See `spec/protocol.md §7`.

Additional requirements there:
- Every emoji shows its text label beneath it
- Word input is case-insensitive and ignores spaces, hyphens, and punctuation
- The code can be regenerated freely; no aggressive countdown pressure
- Generous letter spacing and large type on the code display

## Anti-patterns

```html
<!-- WRONG: no accessible name -->
<button hx-delete="/fill/1"><?= icon('trash') ?></button>
<!-- RIGHT -->
<button hx-delete="/fill/1"><?= icon('trash', 'Delete this fill') ?></button>

<!-- WRONG: placeholder as label, colour-only error -->
<input name="drug" placeholder="Drug name" style="border-color:red">
<!-- RIGHT -->
<label for="drug">Drug name</label>
<input id="drug" name="drug" aria-describedby="drug-err">
<p id="drug-err" role="alert">Enter the drug name.</p>

<!-- WRONG: status by colour -->
<span class="dot red"></span>
<!-- RIGHT -->
<span class="dot red"><?= icon('exclamation-triangle') ?> Overdue</span>

<!-- WRONG: fake table -->
<div class="row"><div class="cell">Drug</div></div>
<!-- RIGHT -->
<table><thead><tr><th scope="col">Drug</th></tr></thead>…</table>
```

## Verify

```bash
privatium lint apps/<slug>          # catches labels, icon names, heading order, contrast
```

Then, by hand — the linter cannot do these:
1. Unplug the mouse. Complete the main task.
2. Turn on the screen reader. Complete the main task.
3. Zoom to 200%. Nothing overlaps or is cut off.
4. Disable images. The pairing flow still works.

Do not present an app as finished before doing all four.
