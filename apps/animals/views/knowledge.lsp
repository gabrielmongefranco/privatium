<?-- Project: Privatium™ | apps/animals/views/knowledge.lsp --?>

<h1>What I know</h1>

<? if #rows == 0 then ?>
  <p class="pv-empty">Nothing yet. <a href="<?= url('/') ?>">Play a round.</a></p>
<? else ?>
  <table>
    <thead><tr><th scope="col">Animal</th><th scope="col">Questions leading here</th></tr></thead>
    <tbody>
    <? for _, r in ipairs(rows) do ?>
      <tr>
        <td><?= r.animal ?></td>
        <td class="pv-path"><?= r.path ~= '' and r.path or '(the first one)' ?></td>
      </tr>
    <? end ?>
    </tbody>
  </table>

  <form method="post" action="<?= url('/reset') ?>"
        onsubmit="return confirm('Forget every animal? Your event history is kept.')">
    <?= csrf() ?>
    <button type="submit" class="pv-btn pv-btn-danger">
      <?= icon('trash') ?> Forget everything
    </button>
  </form>
<? end ?>

<footer class="pv-meta"><a href="<?= url('/') ?>"><?= icon('arrow-left') ?> Back to the game</a></footer>
