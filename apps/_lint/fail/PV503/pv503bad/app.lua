-- Project:  Privatium™  |  File: apps/_lint/fail/PV503/pv503bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV503 fail: the launcher would show question-circle.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
