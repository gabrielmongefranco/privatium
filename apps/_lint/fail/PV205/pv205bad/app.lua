-- Project:  Privatium™  |  File: apps/_lint/fail/PV205/pv205bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV205 fail: the manifest widens sql without a word.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
