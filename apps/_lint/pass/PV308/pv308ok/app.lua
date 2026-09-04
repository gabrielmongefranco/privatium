-- Project:  Privatium™  |  File: apps/_lint/pass/PV308/pv308ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV308 pass: the framework's exact sum and SQLite's date modifier.

local pv = require 'privatium'

pv.get('/', function()
  local row = pv.query1("SELECT decimal_sum(copay) AS total, date(due_on, '+30 days') AS next FROM fill")
  return pv.render('index', { row = row })
end)
