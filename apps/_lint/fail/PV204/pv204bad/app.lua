-- Project:  Privatium™  |  File: apps/_lint/fail/PV204/pv204bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handlers are fine; the template omits the token.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.post('/save', function() return pv.redirect(url('/')) end)
