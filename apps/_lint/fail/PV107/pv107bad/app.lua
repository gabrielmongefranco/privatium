-- Project:  Privatium™  |  File: apps/_lint/fail/PV107/pv107bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  Otherwise a clean app. See main README.md for full license information.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
