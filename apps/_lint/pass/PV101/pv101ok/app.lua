-- Project:  Privatium™  |  File: apps/_lint/pass/PV101/pv101ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV101 pass: the manifest carries slug, title, version, api and tier.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
