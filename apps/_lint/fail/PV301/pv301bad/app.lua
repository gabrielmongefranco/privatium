-- Project:  Privatium™  |  File: apps/_lint/fail/PV301/pv301bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV301 fail: the redirect names the mount, which solo mode has not got.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.post('/save', function() return pv.redirect('/a/pv301bad/') end)
