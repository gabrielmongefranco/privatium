-- Project:  Privatium™  |  File: apps/_lint/pass/PV203/pv203ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV203 pass: os.date and os.time stay in the sandbox.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', { today = os.date('!%Y-%m-%d'), t = os.time() })
end)
