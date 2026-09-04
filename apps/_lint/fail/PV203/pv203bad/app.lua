-- Project:  Privatium™  |  File: apps/_lint/fail/PV203/pv203bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV203 fail: a removed name, reached for anyway.

local pv = require 'privatium'

pv.get('/', function()
  os.execute('ls')
  local f = io.open('notes.txt')
  return pv.render('index', {})
end)
