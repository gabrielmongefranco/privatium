-- Project:  Privatium™  |  File: apps/_lint/pass/PV405/pv405ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV405 pass: the colour is paired with text.
--           See main README.md for full license information.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
