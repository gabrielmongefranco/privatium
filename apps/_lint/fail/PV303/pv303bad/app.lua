-- Project:  Privatium™  |  File: apps/_lint/fail/PV303/pv303bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV303 fail: a write statement where only reads run.

local pv = require 'privatium'

pv.get('/', function()
  return pv.render('index', { rows = pv.query('SELECT id, text FROM note') })
end)
pv.post('/clear', function()
  pv.query('DELETE FROM note')
  return pv.redirect(url('/'))
end)
