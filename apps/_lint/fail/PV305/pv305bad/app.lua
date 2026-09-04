-- Project:  Privatium™  |  File: apps/_lint/fail/PV305/pv305bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV305 fail: a dedupe set by name.

local pv = require 'privatium'

local seen_txid = {}

pv.get('/', function() return pv.render('index', { seen = seen_txid }) end)
