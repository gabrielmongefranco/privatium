-- Project:  Privatium™  |  File: apps/_lint/pass/PV208/pv208ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV208 pass: the seed carries ordinary values.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
