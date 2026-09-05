// Project:  Privatium™  |  File: crates/privatium-core/assets/shell/client.js
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Bootstrap rendering, encrypted HTMX and fresh-document navigation (§8.3).

import { channel, channelFetch, closeChannel, eventSource, localUrl } from './channel.js';
import { base64, decode64 } from './session.js';
import { sha256 } from './vendor/noble/hashes/sha2.js';

export { channel };
/** Fetch on the current authenticated origin without a plaintext fallback. */
export const fetchThroughChannel = channelFetch;
const HANDOFF = 'pv:response';
const uncertainty = () => new Error('The previous response is unavailable. The operation may have completed; check before submitting again.');

/** Save only a response reference and its destination, never form data or response HTML. */
export function saveHandoff(handoff, path, node) {
  if (!/^[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(handoff)) throw uncertainty();
  const url = localUrl(path);
  sessionStorage.setItem(HANDOFF, JSON.stringify({ handoff, path: url.pathname + url.search, node }));
}

/** Remove the reference before attaching; an interrupted attachment is never retried. */
export function takeHandoff(path, node) {
  const raw = sessionStorage.getItem(HANDOFF);
  if (raw === null) return null;
  sessionStorage.removeItem(HANDOFF);
  let value;
  try { value = JSON.parse(raw); } catch { throw uncertainty(); }
  if (value?.path !== path || value.node !== node || !/^[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(value.handoff)) throw uncertainty();
  return value.handoff;
}

/**
 * Replace only an HTMX request's send operation. HTMX retains parameter encoding,
 * validation, indicators, response headers, swapping and completion callbacks.
 */
export function bridgeHtmx(detail, fetcher = channelFetch) {
  const { xhr, requestConfig, pathInfo } = detail;
  const abort = new AbortController();
  xhr.abort = () => { abort.abort(); xhr.onabort?.(); };
  xhr.send = async body => {
    let timer;
    try {
      if (xhr.timeout) timer = setTimeout(() => { abort.abort(); xhr.ontimeout?.(); }, xhr.timeout);
      let response = await fetcher(pathInfo.finalRequestPath, { method: requestConfig.verb.toUpperCase(),
        headers: requestConfig.headers, body: body ?? undefined, signal: abort.signal });
      let destination = localUrl(pathInfo.finalRequestPath);
      for (let count = 0; response.status >= 300 && response.status < 400 && response.headers.has('location'); count++) {
        if (count >= 10 || ![301, 302, 303].includes(response.status)) throw uncertainty();
        destination = localUrl(new URL(response.headers.get('location'), destination));
        await response.body?.cancel(); response = await fetcher(destination.href, { headers: { 'HX-Request': 'true' }, signal: abort.signal });
      }
      let html = await response.text();
      if (globalThis.DOMParser) html = await pinMarkup(html, false, destination.href);
      for (const [name, value] of Object.entries({ status: response.status, statusText: response.statusText,
        response: html, responseText: html, responseURL: destination.href, readyState: 4 })) {
        Object.defineProperty(xhr, name, { configurable: true, value });
      }
      xhr.getResponseHeader = name => response.headers.get(name);
      xhr.getAllResponseHeaders = () => [...response.headers].map(([k, v]) => `${k}: ${v}\r\n`).join('');
      if (!abort.signal.aborted) xhr.onload?.();
    } catch { if (!abort.signal.aborted) xhr.onerror?.(); }
    finally { clearTimeout(timer); }
  };
}

/** Hash a same-origin resource through the channel, or require an authenticated remote hash. */
export async function resourceIntegrity(url, provided, fetcher = channelFetch) {
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('Cannot verify this resource URL. Check the app.');
  if (url.origin !== location.origin) {
    try {
      for (const token of (provided || '').trim().split(/\s+/)) {
        const match = token.match(/^sha(256|384|512)-(.+)$/);
        if (!match) throw new Error();
        decode64(match[2], Number(match[1]) / 8);
      }
    } catch { throw new Error('Cannot verify a remote app resource. The app must provide a valid integrity hash.'); }
    return provided;
  }
  const response = await fetcher(url.href);
  if (!response.ok) throw new Error('Cannot verify an app resource. Reload after the owner checks the app.');
  const hash = sha256.create();
  if (response.body) {
    const reader = response.body.getReader();
    try { for (;;) { const { done, value } = await reader.read(); if (done) break; hash.update(value); } }
    finally { reader.releaseLock(); }
  }
  return 'sha256-' + base64(hash.digest());
}

async function pinMarkup(html, full, base) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  for (const element of doc.querySelectorAll('script[src], link[rel~="stylesheet"][href]')) {
    const attribute = element.tagName === 'SCRIPT' ? 'src' : 'href';
    const url = new URL(element.getAttribute(attribute), base);
    element.setAttribute('integrity', await resourceIntegrity(url, element.getAttribute('integrity')));
    element.setAttribute(attribute, url.href);
    if (url.origin !== location.origin) element.setAttribute('crossorigin', 'anonymous');
  }
  if (!full) return doc.head.innerHTML + doc.body.innerHTML;
  const config = doc.querySelector('meta[name="htmx-config"]') || doc.createElement('meta');
  const settings = config.content ? JSON.parse(config.content) : {};
  config.name = 'htmx-config';
  config.content = JSON.stringify({ ...settings, historyCacheSize: 0, refreshOnHistoryMiss: true }); doc.head.prepend(config);
  return '<!doctype html>' + doc.documentElement.outerHTML;
}

function showFailure(error) {
  const main = document.createElement('main'); main.id = 'main'; main.tabIndex = -1;
  const title = document.createElement('h1'); title.textContent = 'Cannot open this app';
  const message = document.createElement('p'); message.setAttribute('role', 'alert');
  message.textContent = error.message || 'Connection failed. Reconnect and check before submitting again.';
  main.append(title, message);
  if (error.message?.includes('pinned identity')) {
    title.textContent = 'Node identity could not be verified';
    const text = document.createElement('p'); text.textContent = 'Connection stopped. There is no override. Check the node with its owner before forgetting this pairing.';
    const forget = document.createElement('button'); forget.type = 'button'; forget.className = 'pv-btn'; forget.textContent = 'Forget pairing';
    forget.addEventListener('click', () => {
      try { localStorage.removeItem('pv:device'); sessionStorage.removeItem(HANDOFF); location.reload(); }
      catch { message.textContent = 'Cannot clear pairing. Clear this site’s stored data in your browser settings.'; }
    });
    main.append(text, forget);
    if (error.identity) {
      const list = document.createElement('dl'); list.className = 'pv-card';
      for (const [label, value] of [['Node', error.identity.node], ['Pinned key fingerprint', error.identity.pinned], ['Presented key fingerprint', error.identity.presented]]) {
        const term = document.createElement('dt'), description = document.createElement('dd');
        term.textContent = label; description.textContent = value; list.append(term, description);
      }
      main.insertBefore(list, forget);
    }
  }
  document.body.replaceChildren(main); main.focus();
}

function formFailure(error) {
  if (error.identity) { showFailure(error); return; }
  let notice = document.getElementById('pv-channel-error');
  if (!notice) {
    notice = document.createElement('p'); notice.id = 'pv-channel-error'; notice.tabIndex = -1;
    notice.setAttribute('role', 'alert'); (document.querySelector('main') || document.body).prepend(notice);
  }
  notice.textContent = error.message || 'Request failed. Check whether it completed before submitting again.';
  notice.focus();
}

function installNavigation(node) {
  window.addEventListener('pv:identity-refused', event => showFailure(event.detail));
  window.addEventListener('pagehide', closeChannel, { once: true });
  window.addEventListener('pageshow', event => { if (event.persisted) location.reload(); });
  document.addEventListener('htmx:beforeRequest', event => {
    if (event.detail.boosted) {
      event.preventDefault();
      const config = event.detail.requestConfig;
      if (config.verb === 'get') location.assign(localUrl(event.detail.pathInfo.finalRequestPath).href);
      else submit(config.elt.closest('form'), config.triggeringEvent?.submitter, node).catch(formFailure);
    } else bridgeHtmx(event.detail);
  });
  document.addEventListener('submit', event => {
    const form = event.target;
    if (event.defaultPrevented) return;
    event.preventDefault(); submit(form, event.submitter, node).catch(formFailure);
  });
  document.addEventListener('htmx:sendError', () => formFailure(uncertainty()));
}

async function submit(form, button, node) {
  if (!form) throw new Error('Cannot submit this action. Use a form with an accessible submit button.');
  const destination = localUrl(button?.hasAttribute('formaction') ? button.formAction : form.action);
  const method = (button?.getAttribute('formmethod') || form.method || 'get').toUpperCase();
  const values = new FormData(form, button);
  if (method === 'GET') {
    destination.search = new URLSearchParams([...values].map(([key, value]) => [key, typeof value === 'string' ? value : value.name]));
    location.assign(destination.href); return;
  }
  // Check storage before dispatch; a failed handoff must never replay a form (§8.3.1).
  try { sessionStorage.setItem('pv:storage-check', ''); sessionStorage.removeItem('pv:storage-check'); }
  catch { throw new Error('Form was not sent. Enable per-tab browser storage before submitting.'); }
  const encoding = button?.getAttribute('formenctype') || form.enctype;
  const body = encoding === 'multipart/form-data' ? values : new URLSearchParams([...values].map(([key, value]) => [key, typeof value === 'string' ? value : value.name]));
  const connection = await channel();
  const response = await connection.fetch(destination.href, { method, body, navigation: true });
  if ([301, 302, 303].includes(response.status) && response.headers.has('location')) {
    await response.body?.cancel(); location.assign(localUrl(new URL(response.headers.get('location'), destination)).href); return;
  }
  if (!response.handoff) {
    await response.body?.cancel();
    if (response.status === 429) throw new Error('Form was not sent. Finish another navigation before submitting.');
    throw uncertainty();
  }
  try { saveHandoff(response.handoff, destination.href, node); }
  catch { await connection.release(response.handoff); throw uncertainty(); }
  location.assign(destination.href);
}

async function start() {
  const { path, node } = document.body.dataset;
  globalThis.__pv_channel = Object.freeze({ fetch: channelFetch, eventSource });
  const connection = await channel();
  const handoff = takeHandoff(path, node);
  const response = handoff ? await connection.resume(handoff) : await connection.fetch(path, { headers: { accept: 'text/html' } });
  if ([301, 302, 303, 307, 308].includes(response.status) && response.headers.has('location')) {
    await response.body?.cancel(); location.replace(localUrl(new URL(response.headers.get('location'), location.href)).href); return;
  }
  if (!response.headers.get('content-type')?.startsWith('text/html')) {
    const main = document.createElement('main'), heading = document.createElement('h1'), pre = document.createElement('pre');
    if (response.headers.get('content-type') === 'application/zip') {
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement('a'); link.href = url; link.download = 'privatium-skills.zip';
      link.textContent = 'Download skill bundle'; link.className = 'pv-btn'; heading.textContent = 'Skill bundle ready';
      main.append(heading, link); document.body.replaceChildren(main);
      window.addEventListener('pagehide', () => URL.revokeObjectURL(url), { once: true }); return;
    }
    heading.textContent = response.ok ? 'Response' : 'Request could not be completed';
    pre.textContent = await response.text(); main.append(heading, pre); document.body.replaceChildren(main); return;
  }
  const html = await pinMarkup(await response.text(), true, location.href);
  // The bootstrap is already a fresh navigation with the destination app's policy.
  // Parsing once preserves script ordering and the app's DOMContentLoaded lifecycle.
  document.open(); document.write(html); installNavigation(node); document.close();
}

if (globalThis.document?.body?.hasAttribute('data-pv-bootstrap')) start().catch(showFailure);
