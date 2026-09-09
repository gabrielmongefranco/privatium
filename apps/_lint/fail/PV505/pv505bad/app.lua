-- Project:  Privatium™  |  File: apps/_lint/fail/PV505/pv505bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV505 fail: a path that exists on one machine.
--           See main README.md for full license information.

local pv = require 'privatium'

local NOTES = '/home/gabriel/notes.txt'

pv.get('/', function() return pv.render('index', { notes = NOTES }) end)
