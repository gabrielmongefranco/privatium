<?--
This file is part of Privatium
apps/_lint/fail/PV204/pv204bad/views/index.lsp
Summary: PV204 fail: no csrf() in a non-GET form.
Notes: See README file for documentation and full license information.
--?>

<h1>Save</h1>
<form method="post" action="<?= url('/save') ?>">
  <label for="text">Text</label>
  <input id="text" name="text" type="text">
  <button type="submit">Save</button>
</form>
