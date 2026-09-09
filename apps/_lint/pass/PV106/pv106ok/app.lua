-- Project:  Privatium™  |  File: apps/_lint/pass/PV106/pv106ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV106 pass: the schema's table has the row key §4.5 requires.
--           See main README.md for full license information.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
