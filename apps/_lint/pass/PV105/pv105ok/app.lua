-- Project:  Privatium™  |  File: apps/_lint/pass/PV105/pv105ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV105 pass: the tier's required file exists and parses.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
