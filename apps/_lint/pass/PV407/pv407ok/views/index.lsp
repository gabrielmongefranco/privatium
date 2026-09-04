<?-- Project: Privatium™ | apps/_lint/pass/PV407/pv407ok/views/index.lsp
     Summary: PV407 pass: th scope on every header cell. --?>
<h1>Notes</h1>
<table>
  <thead><tr><th scope="col">Note</th></tr></thead>
  <tbody>
  <? for _, r in ipairs(rows) do ?>
    <tr><td><?= r.text ?></td></tr>
  <? end ?>
  </tbody>
</table>
