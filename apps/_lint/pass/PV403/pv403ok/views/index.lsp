<?--
This file is part of Privatium
apps/_lint/pass/PV403/pv403ok/views/index.lsp
Summary: PV403 pass: fieldset and legend around the group.
Notes: See README file for documentation and full license information.
--?>

<h1>Preferences</h1>
<form method="get" action="<?= url('/') ?>">
  <fieldset>
    <legend>Remind me</legend>
    <input id="r-mail" name="remind" type="checkbox" value="mail">
    <label for="r-mail">By mail</label>
    <input id="r-push" name="remind" type="checkbox" value="push">
    <label for="r-push">By push</label>
  </fieldset>
  <button type="submit">Save</button>
</form>
