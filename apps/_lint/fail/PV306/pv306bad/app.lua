-- Project:  Privatium™  |  File: apps/_lint/fail/PV306/pv306bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV306 fail: the second append can fail with the first durable.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.post('/teach', function(req)
  local a = pv.append('node', { text = req.form.a })
  pv.append('node', { text = req.form.b, sibling = a })
  return pv.redirect(url('/'))
end)
