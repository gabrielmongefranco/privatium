<?-- Project: Privatium™ | apps/_lint/pass/PV404/pv404ok/views/play.lsp
     Summary: PV404 pass: the page around the board; the board carries the h1. --?>
<div id="board">
  <?= render('_board', { node = node }) ?>
</div>
<h2>About</h2>
<p>The heading lives in the partial, in every branch.</p>
