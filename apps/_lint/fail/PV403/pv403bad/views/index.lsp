<?-- Project: Privatium™ | apps/_lint/fail/PV403/pv403bad/views/index.lsp
     Summary: PV403 fail: two checkboxes outside any fieldset. --?>
<h1>Preferences</h1>
<form method="get" action="<?= url('/') ?>">
  <input id="r-mail" name="remind" type="checkbox" value="mail">
  <label for="r-mail">By mail</label>
  <input id="r-push" name="remind" type="checkbox" value="push">
  <label for="r-push">By push</label>
  <button type="submit">Save</button>
</form>
