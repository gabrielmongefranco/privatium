// Project:  Privatium™  |  File: apps/_lint/pass/PV504/pv504ok/web/app.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  PV504 pass: the module is the app's own file.

import { pv } from '/static/pv.js';

document.getElementById('out').textContent = String(pv.online);
