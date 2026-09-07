// Project:  Privatium™  |  File: apps/_lint/fail/PV207/pv207bad/web/app.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  PV207 fail: the fetch reaches an origin the CSP will block.
//           See main README.md for full license information.

const out = document.getElementById('out');
const response = await fetch('https://api.example.com/today');
out.textContent = await response.text();
