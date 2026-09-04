-- Project:  Privatium™  |  File: apps/_lint/pass/PV305/pv305ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV305 pass: ids are what make a retry idempotent.

local pv = require 'privatium'

local seen = {}

pv.get('/', function() return pv.render('index', { seen = seen }) end)
