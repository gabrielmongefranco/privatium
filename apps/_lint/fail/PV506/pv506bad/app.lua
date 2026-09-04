-- Project:  Privatium™  |  File: apps/_lint/fail/PV506/pv506bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV506 fail: /settings is the framework's in solo mode.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.get('/settings', function() return pv.render('index', {}) end)
