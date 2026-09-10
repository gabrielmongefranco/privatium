<?--
This file is part of Privatium
apps/animals/views/_assets.lsp
Summary: The three files this app adds to the framework's own. Included at the top of every page
         template rather than injected into <head>, because `defer` makes position
         irrelevant to layout and a Tier 1 app has no reason to own the document shell.
Notes: See README file for documentation and full license information.
--?>

<link rel="stylesheet" href="<?= url('/static/animals.css') ?>">
<noscript><link rel="stylesheet" href="<?= url('/static/nojs.css') ?>"></noscript>
<script defer src="<?= url('/static/animals.js') ?>"></script>
<script defer src="<?= url('/static/alpine-csp.min.js') ?>"></script>
