-- Project:  Privatium™  |  File: apps/_lint/pass/PV505/pv505ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV505 pass: the app names nothing outside its folder.
--           See main README.md for full license information.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', { sheet = url('/static/app.css') }) end)
