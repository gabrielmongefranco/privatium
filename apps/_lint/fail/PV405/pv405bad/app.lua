-- Project:  Privatium™  |  File: apps/_lint/fail/PV405/pv405bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handler is fine; the status is colour alone.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
