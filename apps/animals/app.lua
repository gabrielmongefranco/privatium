-- Project:  Privatium™  |  File: apps/animals/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-08-28  |  Modified: 2026-09-03
-- Summary:  The guess-the-animal game. Demonstrates multi-event atomic writes,
--           recursive SQL, stored session state, and the HTMX/Alpine boundary.

-- Lineage: the "Animal" guessing game from David H. Ahl's BASIC Computer Games (1973),
-- preserved at https://github.com/coding-horror/basic-computer-games (Unlicense).
-- Nothing is copied from that project. The classic implementations keep the tree in
-- memory and lose it on exit; here the tree IS the event log, which is the point.

local pv   = require 'privatium'
local tree = require 'tree'          -- lib/tree.lua

-- Where we are: the cursor if a round is in progress, otherwise the root.
local function here()
  local c = pv.query1('SELECT node_id FROM cursor WHERE id = ?', {'cursor'})
  local id = c and c.node_id or tree.root_id()
  if not id then return nil end
  return pv.get_row('node', id)
end

-- Answer a board request the way the caller asked for it.
--
-- HTMX sets HX-Request, so `req.is_htmx` is true and we return _board.lsp alone —
-- the browser swaps it into #board and keeps scroll position and focus. A plain
-- form post (no JavaScript, or a reader with it off) gets a redirect and a full
-- page, which is the same state by a slower route.
--
-- Both paths are load-bearing. The forms in _board.lsp carry `method` and
-- `action` as well as `hx-post` precisely so this function has something correct
-- to do in either case. Deleting the redirect branch would make the app depend on
-- JavaScript to record a guess, which is not a trade this framework makes.
local function board(req, extra)
  local ctx = { node = here(), stats = tree.stats() }
  for k, v in pairs(extra or {}) do ctx[k] = v end

  if req and req.is_htmx then return pv.render('_board', ctx) end
  if extra and extra.err then return pv.render('play', ctx) end
  return pv.redirect(url('/'))
end

pv.get('/', function()
  return pv.render('play', { node = here(), stats = tree.stats() })
end)

-- Start a fresh round at the root.
pv.post('/start', function(req)
  local root = tree.root_id()
  if root then
    pv.append('cursor', 'cursor', { node_id = root, started = pv.now() })
  end
  return board(req)
end)

-- Walk one step down the tree.
pv.post('/answer', function(req)
  local node = here()
  if not node or node.kind ~= 'q' then return board(req) end

  local next_id = req.form.choice == 'yes' and node.yes_id or node.no_id
  pv.append('cursor', 'cursor', { node_id = next_id, started = pv.now() })
  return board(req)
end)

-- Plant the first animal when the tree is empty.
pv.post('/seed', function(req)
  local animal = tree.clean(req.form.animal)
  if not animal then
    return board(req, { err = 'Name any animal.' })
  end
  pv.append('node', { kind = 'a', text = animal })
  return board(req)
end)

pv.get('/teach', function()
  return pv.render('teach', { node = here() })
end)

-- The guess was wrong. Learn the new animal.
--
-- The classic trick: the leaf we landed on BECOMES the question, keeping its own
-- id, and gains two fresh leaves. The parent is never touched and never has to be
-- found, so this is three events rather than four — and every existing pointer
-- into the tree stays correct.
--
-- Note this route redirects rather than swapping a fragment, even under HTMX.
-- Teaching is a navigation: you came here from the board on a separate page and
-- you are going back to it. Swapping would leave the browser's history pointing
-- at a form the user has already submitted. Not every write wants HTMX.
pv.post('/teach', function(req)
  local node     = here()
  local animal   = tree.clean(req.form.animal)
  local question = tree.clean(req.form.question)
  local yes_new  = req.form.answer == 'yes'

  if not (node and node.kind == 'a') then return pv.redirect(url('/')) end
  if not animal or not question then
    return pv.render('teach', { node = node, err = 'Both fields are required.' })
  end

  pv.batch(function(tx)
    local new_leaf = tx.append('node', { kind = 'a', text = animal })
    local old_leaf = tx.append('node', { kind = 'a', text = node.text })

    tx.append('node', node.id, {
      kind   = 'q',
      text   = question,
      yes_id = yes_new and new_leaf or old_leaf,
      no_id  = yes_new and old_leaf or new_leaf,
    })

    tx.delete('cursor', 'cursor')
  end)

  return pv.redirect(url('/'))
end)

pv.get('/knowledge', function()
  return pv.render('knowledge', { rows = tree.knowledge() })
end)

-- Forgetting is tombstones, never a rewrite. The log still holds every round you
-- ever played; only the materialized tree is emptied. The confirmation step in
-- views/knowledge.lsp is Alpine, because a confirmation is not data.
pv.post('/reset', function()
  pv.batch(function(tx)
    for _, n in ipairs(pv.query('SELECT id FROM node')) do tx.delete('node', n.id) end
    tx.delete('cursor', 'cursor')
  end)
  return pv.redirect(url('/'))
end)
