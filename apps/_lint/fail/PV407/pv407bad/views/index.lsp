<?-- Project: Privatium™ | apps/_lint/fail/PV407/pv407bad/views/index.lsp
     Summary: PV407 fail: no th, so nothing names a column. --?>
<h1>Notes</h1>
<table>
  <? for _, r in ipairs(rows) do ?>
    <tr><td><?= r.text ?></td></tr>
  <? end ?>
</table>
