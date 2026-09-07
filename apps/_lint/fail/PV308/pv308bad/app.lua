-- Project:  Privatium™  |  File: apps/_lint/fail/PV308/pv308bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV308 fail: a float sum, and integer arithmetic on a date.
--           See main README.md for full license information.

local pv = require 'privatium'

pv.get('/', function()
  local row = pv.query1('SELECT SUM(copay) AS total, due_on + 30 AS next FROM fill')
  return pv.render('index', { row = row })
end)
