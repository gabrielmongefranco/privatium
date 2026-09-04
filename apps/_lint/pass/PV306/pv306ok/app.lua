-- Project:  Privatium™  |  File: apps/_lint/pass/PV306/pv306ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV306 pass: pv.batch makes the two events one write.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.post('/teach', function(req)
  pv.batch(function(tx)
    local a = tx.append('node', { text = req.form.a })
    tx.append('node', { text = req.form.b, sibling = a })
  end)
  return pv.redirect(url('/'))
end)
