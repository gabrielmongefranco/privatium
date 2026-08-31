-- Project:  Privatium™  |  File: apps/animals/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-08-28  |  Modified: 2026-08-28
-- Summary:  The guess-the-animal game. Demonstrates multi-event atomic writes,
--           recursive SQL, and stored session state.

local pv   = require 'privatium'
local tree = require 'tree'          -- lib/tree.lua

-- Where we are: the cursor if a round is in progress, otherwise the root.
local function here()
  local c = pv.query1('SELECT node_id FROM cursor WHERE id = ?', {'cursor'})
  local id = c and c.node_id or tree.root_id()
  if not id then return nil end
  return pv.get_row('node', id)
end

pv.get('/', function()
  return pv.render('play', { node = here(), stats = tree.stats() })
end)

-- Start a fresh round at the root.
pv.post('/start', function()
  local root = tree.root_id()
  if root then
    pv.append('cursor', 'cursor', { node_id = root, started = pv.now() })
  end
  return pv.redirect(url('/'))
end)

-- Walk one step down the tree.
pv.post('/answer', function(req)
  local node = here()
  if not node or node.kind ~= 'q' then return pv.redirect(url('/')) end

  local next_id = req.form.choice == 'yes' and node.yes_id or node.no_id
  pv.append('cursor', 'cursor', { node_id = next_id, started = pv.now() })
  return pv.redirect(url('/'))
end)

-- Plant the first animal when the tree is empty.
pv.post('/seed', function(req)
  local animal = tree.clean(req.form.animal)
  if not animal then
    return pv.render('play', { node = nil, error = 'Name any animal.' })
  end
  pv.append('node', { kind = 'a', text = animal })
  return pv.redirect(url('/'))
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
pv.post('/teach', function(req)
  local node     = here()
  local animal   = tree.clean(req.form.animal)
  local question = tree.clean(req.form.question)
  local yes_new  = req.form.answer == 'yes'

  if not (node and node.kind == 'a') then return pv.redirect(url('/')) end
  if not animal or not question then
    return pv.render('teach', { node = node, error = 'Both fields are required.' })
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

pv.post('/reset', function()
  pv.batch(function(tx)
    for _, n in ipairs(pv.query('SELECT id FROM node')) do tx.delete('node', n.id) end
    tx.delete('cursor', 'cursor')
  end)
  return pv.redirect(url('/'))
end)
