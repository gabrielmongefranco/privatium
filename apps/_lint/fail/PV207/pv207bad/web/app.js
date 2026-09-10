// This file is part of Privatium
// apps/_lint/fail/PV207/pv207bad/web/app.js
// Author(s): Gabriel Mongefranco
// Created: 2026-09-05
// Last Modified: 2026-09-05
// Summary: PV207 fail: the fetch reaches an origin the CSP will block.
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

const out = document.getElementById('out');
const response = await fetch('https://api.example.com/today');
out.textContent = await response.text();
