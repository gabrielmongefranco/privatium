// Project:  Privatium™  |  File: apps/_lint/fail/PV206/pv206bad/web/app.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  PV206 fail: markup built from a value.
//           See main README.md for full license information.

import { pv } from '/static/pv.js';

const out = document.getElementById('out');
const node = await pv.node();
out.innerHTML = '<b>' + node.name + '</b>';
