-- Project:  Privatium™  |  File: apps/_lint/pass/PV307/pv307ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV307 pass: a local, not a global, and no load-time table mutated.

local pv = require 'privatium'

local GREETING = 'Hello'

pv.get('/', function()
  local last = os.time()
  return pv.render('index', { greeting = GREETING, last = last })
end)
