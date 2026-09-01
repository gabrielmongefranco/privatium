<?-- Project: Privatium™ | apps/hello/views/edit.lsp
     Summary: The name form. One field, one POST, and the csrf() token PV204
              requires of every non-GET form. --?>

<h1>What should I call you?</h1>

<? if error then ?>
  <p class="pv-error" role="alert"><?= icon('exclamation-triangle') ?> <?= error ?></p>
<? end ?>

<form method="post" action="<?= url('/name') ?>">
  <?= csrf() ?>
  <label for="display_name">Your name</label>
  <input id="display_name" name="display_name" type="text" maxlength="60" required
         autocomplete="name" value="<?= me and me.display_name or '' ?>">
  <p class="pv-help">Stored on this device only. Nobody else will ever see it.</p>

  <button type="submit" class="pv-btn pv-btn-primary">Save</button>
  <a class="pv-btn" href="<?= url('/') ?>">Cancel</a>
</form>
