-- Project:  Privatium™  |  File: apps/_lint/pass/PV503/pv503ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV503 pass: gear exists in Bootstrap Icons.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
