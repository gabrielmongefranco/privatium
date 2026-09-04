-- Project:  Privatium™  |  File: apps/_lint/fail/PV208/pv208bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The app is fine; the seed is not.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
