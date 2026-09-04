-- Project:  Privatium™  |  File: apps/_lint/fail/PV201/pv201bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV201 fail: the query string is concatenated from a request value.

local pv = require 'privatium'

pv.get('/', function(req)
  local rows = pv.query("SELECT id, text FROM note WHERE text = '" .. req.query.q .. "'")
  return pv.render('index', { rows = rows })
end)
