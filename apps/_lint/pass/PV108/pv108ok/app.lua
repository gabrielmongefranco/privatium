-- Project:  Privatium™  |  File: apps/_lint/pass/PV108/pv108ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV108 pass: a plain index beside the primary key; otherwise a clean app.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
