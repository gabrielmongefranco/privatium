-- Project:  Privatium™  |  File: apps/_lint/pass/PV103/pv103ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV103 pass: api = 1.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
