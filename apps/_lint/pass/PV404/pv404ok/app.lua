-- Project:  Privatium™  |  File: apps/_lint/pass/PV404/pv404ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV404 pass: play renders the board; the board answers htmx on its own.

local pv = require 'privatium'

pv.get('/', function(req)
  local ctx = { node = pv.query1('SELECT id, text FROM node LIMIT 1') }
  if req.is_htmx then return pv.render('_board', ctx) end
  return pv.render('play', ctx)
end)
