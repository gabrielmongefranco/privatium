<?-- Project: Privatium™ | apps/animals/views/_board.lsp
     Summary: The game board, on its own so HTMX can swap it.

     THIS IS THE HTMX HALF OF THE APP. Everything in here changes data: a guess
     answered, an animal planted, a round restarted. Each one is a form that posts
     and is replaced by the server's rendering of the new state.

     Every form works with JavaScript switched off. `hx-post` is an enhancement
     layered on `method`/`action`, not a replacement for them — HTMX intercepts
     the submit when it can and the browser handles it when it cannot. Keep both. --?>

<? if error then ?>
  <p class="pv-error" role="alert"><?= icon('exclamation-triangle') ?> <?= error ?></p>
<? end ?>

<? if not node then ?>

  <h1>I don't know any animals yet.</h1>
  <?-- hx-target="#board": the server returns this partial and it replaces itself. --?>
  <form method="post" action="<?= url('/seed') ?>"
        hx-post="<?= url('/seed') ?>" hx-target="#board">
    <?= csrf() ?>
    <label for="animal">Name one</label>
    <input id="animal" name="animal" type="text" maxlength="40"
           placeholder="elephant" required autofocus>
    <button type="submit" class="pv-btn pv-btn-primary">Start</button>
  </form>

<? elseif node.kind == 'q' then ?>

  <h1><?= node.text ?></h1>
  <div class="pv-actions">
    <form method="post" action="<?= url('/answer') ?>"
          hx-post="<?= url('/answer') ?>" hx-target="#board">
      <?= csrf() ?><input type="hidden" name="choice" value="yes">
      <button type="submit" class="pv-btn pv-btn-primary">
        <?= icon('hand-thumbs-up') ?> Yes
      </button>
    </form>
    <form method="post" action="<?= url('/answer') ?>"
          hx-post="<?= url('/answer') ?>" hx-target="#board">
      <?= csrf() ?><input type="hidden" name="choice" value="no">
      <button type="submit" class="pv-btn">
        <?= icon('hand-thumbs-down') ?> No
      </button>
    </form>
  </div>

<? else ?>

  <h1>Is it a <?= node.text ?>?</h1>
  <div class="pv-actions">
    <form method="post" action="<?= url('/start') ?>"
          hx-post="<?= url('/start') ?>" hx-target="#board">
      <?= csrf() ?>
      <button type="submit" class="pv-btn pv-btn-primary">
        <?= icon('check-lg') ?> Yes — you got it
      </button>
    </form>
    <?-- A plain link, deliberately. Teaching is a different page with its own
         form and its own back button; swapping it into #board would break the
         browser's history for no gain. Not every interaction wants HTMX. --?>
    <a class="pv-btn" href="<?= url('/teach') ?>"><?= icon('x-lg') ?> No</a>
  </div>

<? end ?>

<? if stats then ?>
  <p class="pv-meta"><?= stats.animals ?> animals, <?= stats.questions ?> questions</p>
<? end ?>
