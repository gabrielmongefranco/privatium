-- Project:  Privatium™  |  File: apps/hello/app.lua
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-08-28  |  Modified: 2026-09-03
-- Summary:  The entire application. Two routes, eleven lines of logic.

local pv = require 'privatium'

local function profile()
  return pv.query1('SELECT id, display_name FROM profile LIMIT 1')
end

pv.get('/', function()
  return pv.render('index', { me = profile() })
end)

pv.get('/edit', function()
  return pv.render('edit', { me = profile() })
end)

pv.post('/name', function(req)
  local name = (req.form.display_name or ''):gsub('^%s+', ''):gsub('%s+$', '')
  if name == '' then
    return pv.render('edit', { me = profile(), err = 'Please enter a name.' })
  end

  local me = profile()
  -- Reusing the existing id makes this an amendment, not a second person.
  pv.append('profile', me and me.id or nil, { display_name = name })

  return pv.redirect(url('/'))
end)
