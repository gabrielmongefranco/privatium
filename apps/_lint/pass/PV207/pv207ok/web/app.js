// Project:  Privatium™  |  File: apps/_lint/pass/PV207/pv207ok/web/app.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  PV207 pass: the origin the fetch reaches is in permissions.remote.

const out = document.getElementById('out');
const response = await fetch('https://api.example.com/today');
out.textContent = await response.text();
