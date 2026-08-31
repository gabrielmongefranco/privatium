<?-- Project: Privatium™ | apps/animals/views/teach.lsp --?>

<h1>I give up. What was it?</h1>

<? if error then ?>
  <p class="pv-error" role="alert"><?= icon('exclamation-triangle') ?> <?= error ?></p>
<? end ?>

<form method="post" action="<?= url('/teach') ?>">
  <?= csrf() ?>

  <label for="animal">Your animal</label>
  <input id="animal" name="animal" type="text" maxlength="40"
         placeholder="wombat" required autofocus>

  <label for="question">
    A yes/no question that tells it apart from <?= node and node.text or 'my guess' ?>
  </label>
  <input id="question" name="question" type="text" maxlength="120"
         placeholder="Does it have cubic droppings?" required>
  <p class="pv-help">Anything true for one animal and false for the other.</p>

  <fieldset>
    <legend>And for your animal, the answer is</legend>
    <label><input type="radio" name="answer" value="yes" checked> Yes</label>
    <label><input type="radio" name="answer" value="no"> No</label>
  </fieldset>

  <button type="submit" class="pv-btn pv-btn-primary">Teach me</button>
  <a class="pv-btn" href="<?= url('/') ?>">Cancel</a>
</form>
