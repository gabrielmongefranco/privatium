-- This file is part of Privatium
-- apps/_lint/pass/PV306/pv306ok/app.lua
-- Author(s): Gabriel Mongefranco
-- Created: 2026-09-05
-- Last Modified: 2026-09-05
-- Summary: PV306 pass: pv.batch makes the two events one write.
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
  pv.batch(function(tx)
    local a = tx.append('node', { text = req.form.a })
    tx.append('node', { text = req.form.b, sibling = a })
  end)
  return pv.redirect(url('/'))
end)
