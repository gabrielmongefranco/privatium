-- Project:  Privatium™  |  File: apps/_lint/pass/PV401/pv401ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV401 pass: the icon names what the control does.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
