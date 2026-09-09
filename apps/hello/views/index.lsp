<?-- Project: Privatium™ | apps/hello/views/index.lsp
     Summary: Greeting. Note <?= ?> escapes by default, so a name containing markup is
              displayed, never executed. See main README.md for full license information. --?>

<link rel="stylesheet" href="<?= url('/static/hello.css') ?>">
<div class="hello">
<p class="hello-mark"><?= icon('chat-heart') ?></p>
<? if not me then ?>
  <h1>We haven't met yet.</h1>
  <p>Give me a name, and I'll remember it for next time.</p>
  <a class="pv-btn pv-btn-primary" href="<?= url('/edit') ?>">
    <?= icon('chat-heart') ?> Introduce yourself
  </a>
<? else ?>
  <h1>
    <? local h = tonumber(os.date('!%H')) ?>
    <?= h < 12 and 'Good morning' or h < 18 and 'Good afternoon' or 'Good evening' ?>,
    <?= me.display_name ?>.
  </h1>
  <a class="pv-btn" href="<?= url('/edit') ?>">
    <?= icon('pencil') ?> Change my name
  </a>
<? end ?>
</div>
