-- Project:  Privatium™  |  File: apps/_lint/fail/PV401/pv401bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  The handler is fine; the template's control has no name.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
