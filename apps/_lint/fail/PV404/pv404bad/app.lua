-- Project:  Privatium™  |  File: apps/_lint/fail/PV404/pv404bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handler is fine; the template's headings are not.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
