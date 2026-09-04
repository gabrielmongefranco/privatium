-- Project:  Privatium™  |  File: apps/_lint/pass/PV204/pv204ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV204 pass: the form carries the token.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.post('/save', function() return pv.redirect(url('/')) end)
