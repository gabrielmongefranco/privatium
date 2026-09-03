-- Project:  Privatium™  |  File: apps/animals/lib/tree.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-08-28  |  Modified: 2026-09-03
-- Summary:  Queries over the decision tree. Kept out of app.lua so the routes
--           stay readable — the same split any growing app should make.

local pv = require 'privatium'
local M  = {}

-- Trim and reject empty input. Returns nil when there is nothing usable.
function M.clean(s)
  s = (s or ''):gsub('^%s+', ''):gsub('%s+$', '')
  return s ~= '' and s or nil
end

-- The root is the only node nobody points at.
function M.root_id()
  local r = pv.query1([[
    SELECT n.id FROM node n
    WHERE NOT EXISTS (SELECT 1 FROM node p WHERE p.yes_id = n.id OR p.no_id = n.id)
    LIMIT 1
  ]])
  return r and r.id
end

-- Every animal with the questions that lead to it.
--
-- `id` is selected as well as `text` because views/knowledge.lsp needs a stable,
-- unique value for the `aria-controls` / `id` pair on each collapsible path. Two
-- animals can share a name in a tree that has been reset and rebuilt; ULIDs
-- cannot, so the accessible name never points at the wrong element.
function M.knowledge()
  return pv.query([[
    WITH RECURSIVE walk(id, depth, path) AS (
      SELECT n.id, 0, ''
      FROM node n
      WHERE NOT EXISTS (SELECT 1 FROM node p WHERE p.yes_id = n.id OR p.no_id = n.id)

      UNION ALL

      SELECT c.id,
             w.depth + 1,
             w.path || CASE WHEN w.path = '' THEN '' ELSE ' -> ' END
                    || p.text || ': ' || t.answer
      FROM walk w
      JOIN node p ON p.id = w.id AND p.kind = 'q'
      CROSS JOIN (SELECT 'yes' AS answer UNION ALL SELECT 'no') AS t
      JOIN node c ON c.id = CASE WHEN t.answer = 'yes' THEN p.yes_id ELSE p.no_id END
    )
    SELECT w.depth, w.path, n.id AS animal_id, n.text AS animal
    FROM walk w JOIN node n ON n.id = w.id
    WHERE n.kind = 'a'
    ORDER BY w.depth, n.text
  ]])
end

function M.stats()
  return pv.query1([[
    SELECT count(*) FILTER (WHERE kind = 'a') AS animals,
           count(*) FILTER (WHERE kind = 'q') AS questions
    FROM node
  ]])
end

return M
