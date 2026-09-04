<?-- Project: Privatium™ | apps/animals/views/_assets.lsp
     Summary: The three files this app adds to the framework's own. Included at the
              top of every page template rather than injected into <head>, because
              `defer` makes position irrelevant to layout and a Tier 1 app has no reason
              to own the document shell.

              HTMX is the framework's, already loaded — do not vendor a second copy.
              Alpine is this app's, and must be the CSP build (see static/VENDOR.md).

              ORDER MATTERS: animals.js comes BEFORE Alpine. Alpine's CDN builds call
              Alpine.start() in a microtask the moment their script runs, and start()
              dispatches `alpine:init` right then — so a component registered from a
              listener in a script that loads after Alpine is registered too late, and
              every x-data on the page is an "Undefined variable". Both scripts are
              `defer`, which runs them in document order; this order is what makes the
              components exist when Alpine looks for them (found in M10, in a browser).

              The <noscript> sheet is the no-JavaScript path: it reverts x-cloak and hides
              the buttons that only toggle Alpine state, so every question path and the
              reset form are simply on the page. A link, not an inline <style>, because
              the default CSP has no style-src (spec/protocol.md §9.3). --?>

<link rel="stylesheet" href="<?= url('/static/animals.css') ?>">
<noscript><link rel="stylesheet" href="<?= url('/static/nojs.css') ?>"></noscript>
<script defer src="<?= url('/static/animals.js') ?>"></script>
<script defer src="<?= url('/static/alpine-csp.min.js') ?>"></script>
