-- Project:  Privatium™  |  File: apps/_lint/pass/PV201/pv201ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV201 pass: the value is bound with ?, never concatenated.

local pv = require 'privatium'

pv.get('/', function(req)
  local rows = pv.query('SELECT id, text FROM note WHERE text = ?', { req.query.q })
  return pv.render('index', { rows = rows })
end)
