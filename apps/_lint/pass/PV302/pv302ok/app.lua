-- Project:  Privatium™  |  File: apps/_lint/pass/PV302/pv302ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV302 pass: pv.dec keeps the digits.

local pv = require 'privatium'

pv.get('/', function()
  local row = pv.query1('SELECT id, copay FROM fill LIMIT 1')
  local total = row and (pv.dec(row.copay) + pv.dec('1.00')) or nil
  return pv.render('index', { total = total })
end)
