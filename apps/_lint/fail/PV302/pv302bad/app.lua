-- Project:  Privatium™  |  File: apps/_lint/fail/PV302/pv302bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV302 fail: money through a double.

local pv = require 'privatium'

pv.get('/', function()
  local row = pv.query1('SELECT id, copay FROM fill LIMIT 1')
  local total = row and (tonumber(row.copay) + 1) or nil
  return pv.render('index', { total = total })
end)
