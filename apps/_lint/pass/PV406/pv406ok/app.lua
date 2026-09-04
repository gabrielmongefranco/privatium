-- Project:  Privatium™  |  File: apps/_lint/pass/PV406/pv406ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV406 pass: the app's own tokens clear the floors.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
