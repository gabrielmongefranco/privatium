<?--
This file is part of Privatium
apps/_lint/pass/PV401/pv401ok/views/index.lsp
Summary: PV401 pass: a labelled icon, and an inline svg that is hidden and unfocusable.
Notes: See README file for documentation and full license information.
--?>

<h1>Notes</h1>
<a class="pv-btn" href="<?= url('/edit') ?>"><?= icon('pencil', 'Edit the note') ?></a>
<svg aria-hidden="true" focusable="false" viewBox="0 0 16 16"><path d="M0 0h16v16H0z"/></svg>
