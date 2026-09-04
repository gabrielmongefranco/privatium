-- Project:  Privatium™  |  File: apps/_lint/fail/PV307/pv307bad/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV307 fail: neither persists the way the author expects.

local pv = require 'privatium'

local cache = {}

pv.get('/', function()
  last_seen = os.time()
  cache['hit'] = (cache['hit'] or 0) + 1
  return pv.render('index', { last = last_seen })
end)
