<?-- Project: Privatium™ | apps/animals/views/knowledge.lsp
     Summary: Everything the app has learned, and the button that forgets it.

     THIS IS WHERE BOTH TOOLS SIT SIDE BY SIDE, which is most of why this page is
     worth reading:

       - Forgetting is a form post. It writes tombstones. HTMX territory.
       - Expanding a question path, and confirming before forgetting, change
         nothing and are worth nothing after a refresh. Alpine territory.

     The test is not "is it interactive". It is: if the user hit reload right now,
     would they lose anything they meant to keep?

     And the corollary: everything Alpine hides must still be reachable with
     JavaScript off. The toggles carry `pv-js-only` and the hidden parts carry
     `x-cloak`; static/nojs.css (loaded from <noscript>) drops the former and
     reveals the latter, so the paths are simply printed and the reset form is
     simply there. --?>

<?= render('_assets') ?>

<h1>What I know</h1>

<? if #rows == 0 then ?>
  <p class="pv-empty">Nothing yet. <a href="<?= url('/') ?>">Play a round.</a></p>
<? else ?>
  <table>
    <thead>
      <tr>
        <th scope="col">Animal</th>
        <th scope="col">Questions leading here</th>
      </tr>
    </thead>
    <tbody>
    <? for _, r in ipairs(rows) do ?>
      <tr>
        <td><?= r.animal ?></td>
        <? if r.path == '' then ?>
          <td class="pv-path">(the first one)</td>
        <? else ?>
          <?-- ALPINE. Collapsed by default so the table stays scannable; paths get
               long fast. `disclosure` is registered in static/animals.js — the CSP
               build cannot evaluate an inline x-data object, and this app does not
               grant itself 'unsafe-eval' to get one. --?>
          <td class="pv-path" x-data="disclosure">
            <button type="button" class="pv-btn pv-btn-quiet pv-js-only"
                    x-on:click="toggle"
                    x-bind:aria-expanded="open"
                    aria-controls="path-<?= r.animal_id ?>">
              <span x-text="label">Show</span> path
            </button>
            <span id="path-<?= r.animal_id ?>" x-show="open" x-cloak>
              <?= r.path ?>
            </span>
          </td>
        <? end ?>
      </tr>
    <? end ?>
    </tbody>
  </table>

  <?-- ALPINE for the confirmation, HTMX-shaped form for the write.

       The old version of this used onsubmit="return confirm(...)". That was a bug
       rather than a preference: an inline handler is script, this app's CSP has no
       'unsafe-inline', and it would never have run. Two states in a component,
       one plain form underneath, nothing persisted either way. With JavaScript
       off the first button is gone and the form is shown: one step instead of
       two, and the write is still yours to make. --?>
  <div x-data="confirmable">
    <button type="button" class="pv-btn pv-btn-danger pv-js-only" x-on:click="ask">
      <?= icon('trash') ?> Forget everything
    </button>

    <div x-show="asking" x-cloak role="group"
         aria-label="Confirm forgetting every animal">
      <p>Forget every animal? Your event history is kept either way — reset writes
         tombstones, it never rewrites a log.</p>
      <form method="post" action="<?= url('/reset') ?>">
        <?= csrf() ?>
        <button type="submit" class="pv-btn pv-btn-danger">Yes, forget them</button>
        <button type="button" class="pv-btn pv-js-only" x-on:click="cancel">Keep them</button>
      </form>
    </div>
  </div>
<? end ?>

<footer class="pv-meta">
  <a href="<?= url('/') ?>"><?= icon('arrow-left') ?> Back to the game</a>
</footer>
