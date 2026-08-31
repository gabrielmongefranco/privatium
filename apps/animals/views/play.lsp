<?-- Project: Privatium™ | apps/animals/views/play.lsp
     Summary: The page around the board. The board itself is _board.lsp, because
              HTMX swaps it and a full page reload would lose the scroll position
              and the focus ring for no reason.

              Nothing on this page is Alpine: every control here writes an event. --?>

<?= render('_assets') ?>

<div id="board">
  <?= render('_board', { node = node, stats = stats, error = error }) ?>
</div>

<footer class="pv-meta">
  <a href="<?= url('/knowledge') ?>"><?= icon('list-ul') ?> What I know</a>
</footer>
