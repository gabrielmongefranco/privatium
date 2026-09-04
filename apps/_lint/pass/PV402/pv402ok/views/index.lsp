<?-- Project: Privatium™ | apps/_lint/pass/PV402/pv402ok/views/index.lsp
     Summary: PV402 pass: a labelled input. --?>
<h1>Search</h1>
<form method="get" action="<?= url('/') ?>">
  <label for="q">Search</label>
  <input id="q" name="q" type="search">
  <button type="submit">Go</button>
</form>
