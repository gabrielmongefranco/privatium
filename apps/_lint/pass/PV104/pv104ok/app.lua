-- Project:  Privatium™  |  File: apps/_lint/pass/PV104/pv104ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV104 pass: folder and slug agree.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
