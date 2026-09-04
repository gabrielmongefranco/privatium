-- Project:  Privatium™  |  File: apps/_lint/pass/PV303/pv303ok/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV303 pass: the write is pv.append.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', { rows = pv.query('SELECT id, text FROM note') })
end)
pv.post('/save', function(req)
  pv.append('note', { text = req.form.text })
  return pv.redirect(url('/'))
end)
