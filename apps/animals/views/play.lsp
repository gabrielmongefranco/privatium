<?-- Project: Privatium™ | apps/animals/views/play.lsp
     Summary: The game board. HTMX posts, no custom JavaScript. --?>

<? if error then ?>
  <p class="pv-error" role="alert"><?= icon('exclamation-triangle') ?> <?= error ?></p>
<? end ?>

<? if not node then ?>

  <h1>I don't know any animals yet.</h1>
  <form method="post" action="<?= url('/seed') ?>">
    <?= csrf() ?>
    <label for="animal">Name one</label>
    <input id="animal" name="animal" type="text" maxlength="40"
           placeholder="elephant" required autofocus>
    <button type="submit" class="pv-btn pv-btn-primary">Start</button>
  </form>

<? elseif node.kind == 'q' then ?>

  <h1><?= node.text ?></h1>
  <div class="pv-actions">
    <form method="post" action="<?= url('/answer') ?>">
      <?= csrf() ?><input type="hidden" name="choice" value="yes">
      <button type="submit" class="pv-btn pv-btn-primary">
        <?= icon('hand-thumbs-up') ?> Yes
      </button>
    </form>
    <form method="post" action="<?= url('/answer') ?>">
      <?= csrf() ?><input type="hidden" name="choice" value="no">
      <button type="submit" class="pv-btn">
        <?= icon('hand-thumbs-down') ?> No
      </button>
    </form>
  </div>

<? else ?>

  <h1>Is it a <?= node.text ?>?</h1>
  <div class="pv-actions">
    <form method="post" action="<?= url('/start') ?>">
      <?= csrf() ?>
      <button type="submit" class="pv-btn pv-btn-primary">
        <?= icon('check-lg') ?> Yes — you got it
      </button>
    </form>
    <a class="pv-btn" href="<?= url('/teach') ?>"><?= icon('x-lg') ?> No</a>
  </div>

<? end ?>

<footer class="pv-meta">
  <a href="<?= url('/knowledge') ?>"><?= icon('list-ul') ?> What I know</a>
  <? if stats then ?>
    <span><?= stats.animals ?> animals, <?= stats.questions ?> questions</span>
  <? end ?>
</footer>
