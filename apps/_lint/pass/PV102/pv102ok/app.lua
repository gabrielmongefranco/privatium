-- Project:  Privatium™  |  File: apps/_lint/pass/PV102/pv102ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV102 pass: the slug matches the pattern and is not reserved.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
