-- Project:  Privatium™  |  File: apps/_lint/pass/PV407/pv407ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV407 pass: tabular data in a table.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', { rows = pv.query('SELECT id, text FROM note') })
end)
