-- Project:  Privatium™  |  File: apps/_lint/pass/PV402/pv402ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV402 pass: label for names the input.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
