-- Project:  Privatium™  |  File: apps/_lint/pass/PV202/pv202ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV202 pass: every emit is <?= ?>.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
