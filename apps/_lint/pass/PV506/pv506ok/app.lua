-- Project:  Privatium™  |  File: apps/_lint/pass/PV506/pv506ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV506 pass: /prefs is the app's in both modes.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.get('/prefs', function() return pv.render('index', {}) end)
