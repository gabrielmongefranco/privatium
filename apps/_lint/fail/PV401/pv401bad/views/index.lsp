<?-- Project: Privatium™ | apps/_lint/fail/PV401/pv401bad/views/index.lsp
     Summary: PV401 fail: nothing for a screen reader to announce; the svg is also focusable. --?>
<h1>Notes</h1>
<a class="pv-btn" href="<?= url('/edit') ?>"><?= icon('pencil') ?></a>
<svg aria-hidden="true" viewBox="0 0 16 16"><path d="M0 0h16v16H0z"/></svg>
