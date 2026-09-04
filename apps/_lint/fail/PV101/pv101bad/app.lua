-- Project:  Privatium™  |  File: apps/_lint/fail/PV101/pv101bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handler is fine; the manifest is not.

local pv = require 'privatium'

pv.get('/', function() return 'ok' end)
