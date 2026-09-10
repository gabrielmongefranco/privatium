<?--
This file is part of Privatium
apps/_lint/pass/PV402/pv402ok/views/index.lsp
Summary: PV402 pass: a labelled input.
Notes: See README file for documentation and full license information.
--?>

<h1>Search</h1>
<form method="get" action="<?= url('/') ?>">
  <label for="q">Search</label>
  <input id="q" name="q" type="search">
  <button type="submit">Go</button>
</form>
