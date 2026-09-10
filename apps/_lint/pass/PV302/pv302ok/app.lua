-- This file is part of Privatium
-- apps/_lint/pass/PV302/pv302ok/app.lua
-- Author(s): Gabriel Mongefranco
-- Created: 2026-09-05
-- Last Modified: 2026-09-05
-- Summary: PV302 pass: pv.dec keeps the digits.
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

pv.get('/', function()
  local row = pv.query1('SELECT id, copay FROM fill LIMIT 1')
  local total = row and (pv.dec(row.copay) + pv.dec('1.00')) or nil
  return pv.render('index', { total = total })
end)
