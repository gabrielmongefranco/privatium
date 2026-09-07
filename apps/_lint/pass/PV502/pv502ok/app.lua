-- Project:  Privatium™  |  File: apps/_lint/pass/PV502/pv502ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV502 pass: the node this fixture is linted under runs solo with this app at /.
--           See main README.md for full license information.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
