<?-- Project: Privatium™ | apps/hello/views/index.lsp
     Summary: Greeting. Note <?= ?> escapes by default, so a name containing
              markup is displayed, never executed. --?>

<? if not me then ?>
  <p class="pv-empty">We haven't met yet.</p>
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
