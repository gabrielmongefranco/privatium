-- Project:  Privatium™  |  File: apps/_lint/fail/PV502/pv502bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV502 fail: host mode is the default, and the loader refuses this there.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
