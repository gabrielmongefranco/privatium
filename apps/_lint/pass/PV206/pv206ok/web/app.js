// Project:  Privatium™  |  File: apps/_lint/pass/PV206/pv206ok/web/app.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  PV206 pass: data goes through textContent, never innerHTML.

import { pv } from '/static/pv.js';

const out = document.getElementById('out');
const node = await pv.node();
out.textContent = node.name;
