-- Project:  Privatium™  |  File: apps/_lint/fail/PV403/pv403bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handler is fine; the group has no name.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
