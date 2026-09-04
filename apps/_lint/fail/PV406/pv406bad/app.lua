-- Project:  Privatium™  |  File: apps/_lint/fail/PV406/pv406bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The app is fine; its stylesheet's tokens are not.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
