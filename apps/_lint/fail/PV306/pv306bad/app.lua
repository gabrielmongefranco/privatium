-- This file is part of Privatium
-- apps/_lint/fail/PV306/pv306bad/app.lua
-- Author(s): Gabriel Mongefranco
-- Created: 2026-09-05
-- Last Modified: 2026-09-05
-- Summary: PV306 fail: the second append can fail with the first durable.
-- Notes: See README file for documentation and full license information.
--
-- Copyright © 2026 Gabriel Mongefranco
--
-- This program is free software: you can redistribute it and/or modify
-- it under the terms of the GNU General Public License as published by
-- the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
--
-- This program is distributed in the hope that it will be useful,
-- but WITHOUT ANY WARRANTY; without even the implied warranty of
-- MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
-- GNU General Public License for more details.
--
-- You should have received a copy of the GNU General Public License along
-- with this program. If not, see <https://www.gnu.org/licenses/>.

local pv = require 'privatium'

pv.get('/', function() return pv.render('index', {}) end)
pv.post('/teach', function(req)
  local a = pv.append('node', { text = req.form.a })
  pv.append('node', { text = req.form.b, sibling = a })
  return pv.redirect(url('/'))
end)
