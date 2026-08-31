<?-- Project: Privatium™ | apps/animals/views/_assets.lsp
     Summary: The two files this app adds to the framework's own. Included at the
              top of every page template rather than injected into <head>, because
              `defer` makes position irrelevant and a Tier 1 app has no reason to
              own the document shell.

              HTMX is the framework's, already loaded — do not vendor a second copy.
              Alpine is this app's, and must be the CSP build (see static/VENDOR.md). --?>

<link rel="stylesheet" href="<?= url('/static/animals.css') ?>">
<script defer src="<?= url('/static/alpine-csp.min.js') ?>"></script>
<script defer src="<?= url('/static/animals.js') ?>"></script>
