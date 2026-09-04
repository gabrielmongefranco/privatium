-- Project:  Privatium™  |  File: apps/_lint/fail/PV407/pv407bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handler is fine; the table has no th.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', { rows = pv.query('SELECT id, text FROM note') })
end)
