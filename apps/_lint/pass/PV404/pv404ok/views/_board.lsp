<?--
This file is part of Privatium
apps/_lint/pass/PV404/pv404ok/views/_board.lsp
Summary: PV404 pass: each state of the board supplies the page's one h1.
Notes: See README file for documentation and full license information.
--?>

<? if not node then ?>
  <h1>Nothing yet</h1>
<? else ?>
  <h1><?= node.text ?></h1>
<? end ?>
