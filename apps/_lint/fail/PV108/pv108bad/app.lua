-- Project:  Privatium™  |  File: apps/_lint/fail/PV108/pv108bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  Otherwise a clean app.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
