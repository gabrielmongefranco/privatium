<?-- Project: Privatium™ | apps/hello/views/edit.lsp
     Summary: The name form. One field, one POST, and the csrf() token PV204
              requires of every non-GET form. --?>

<link rel="stylesheet" href="<?= url('/static/hello.css') ?>">
<div class="hello">
<h1>What should I call you?</h1>

<? if err then ?>
  <p class="pv-error" role="alert"><?= icon('exclamation-triangle') ?> <?= err ?></p>
<? end ?>

<form method="post" action="<?= url('/name') ?>">
  <?= csrf() ?>
  <label for="display_name">Your name</label>
  <input id="display_name" name="display_name" type="text" maxlength="60" required autofocus
         autocomplete="name" aria-describedby="name-help" value="<?= me and me.display_name or '' ?>">
  <p id="name-help" class="pv-help">Use your name or a nickname. You can change it whenever you like.</p>

  <button type="submit" class="pv-btn pv-btn-primary">Save</button>
  <a class="pv-btn" href="<?= url('/') ?>">Cancel</a>
</form>
</div>
