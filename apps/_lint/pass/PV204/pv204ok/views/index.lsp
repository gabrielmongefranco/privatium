<?-- Project: Privatium™ | apps/_lint/pass/PV204/pv204ok/views/index.lsp
     Summary: PV204 pass: csrf() inside the non-GET form. --?>
<h1>Save</h1>
<form method="post" action="<?= url('/save') ?>">
  <?= csrf() ?>
  <label for="text">Text</label>
  <input id="text" name="text" type="text">
  <button type="submit">Save</button>
</form>
