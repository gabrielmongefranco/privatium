-- Project:  Privatium™  |  File: apps/_lint/pass/PV403/pv403ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV403 pass: the group is named by its legend.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
