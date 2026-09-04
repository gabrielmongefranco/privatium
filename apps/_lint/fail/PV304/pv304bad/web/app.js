// Project:  Privatium™  |  File: apps/_lint/fail/PV304/pv304bad/web/app.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  PV304 fail: envelope fields set client-side; the server rejects them.

import { pv } from '/static/pv.js';

await pv.append([{ op: 'put', tbl: 'stroke', id: pv.ulid(), d: { points: [] }, seq: 1, ts: new Date().toISOString() }]);
