<?--
This file is part of Privatium
apps/_lint/fail/PV402/pv402bad/views/index.lsp
Summary: PV402 fail: placeholder is not a label.
Notes: See README file for documentation and full license information.
--?>

<h1>Search</h1>
<form method="get" action="<?= url('/') ?>">
  <input id="q" name="q" type="search" placeholder="Search">
  <button type="submit">Go</button>
</form>
