-- Project:  Privatium™  |  File: apps/_lint/pass/PV107/pv107ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV107 pass: CREATE TABLE, CREATE VIEW, CREATE INDEX and comments.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', {})
end)
