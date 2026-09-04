-- Project:  Privatium™  |  File: apps/_lint/pass/PV301/pv301ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV301 pass: url() builds the path, so solo mode works.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.post('/save', function() return pv.redirect(url('/')) end)
