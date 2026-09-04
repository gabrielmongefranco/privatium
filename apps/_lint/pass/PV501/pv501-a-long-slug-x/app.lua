-- Project:  Privatium™  |  File: apps/_lint/pass/PV501/pv501-a-long-slug-x/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV501 pass: longer than a DNS-SD label, and not advertised.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
