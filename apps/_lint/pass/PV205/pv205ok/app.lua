-- Project:  Privatium™  |  File: apps/_lint/pass/PV205/pv205ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV205 pass: the permission says why.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
