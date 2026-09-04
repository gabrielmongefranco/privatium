-- Project:  Privatium™  |  File: apps/_lint/fail/PV202/pv202bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handler is fine; the template emits raw.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
