#!/usr/bin/env node
// Simulador MÍNIMO de la API del Mail Server de aaPanel para desarrollo y
// pruebas. Verifica la firma (request_time/request_token) como el panel real
// y guarda dominios y buzones en memoria. Crea los buzones en GreenMail para
// que se pueda iniciar sesión por IMAP con ellos.
//   node scripts/fake-aapanel.mjs [puerto] [clave]
import { createHash } from 'node:crypto';
import http from 'node:http';

const PORT = Number(process.argv[2] ?? 7800);
const KEY = process.argv[3] ?? 'dev-key';
const GREENMAIL = process.env.GREENMAIL_API ?? 'http://localhost:8080';
const md5 = (s) => createHash('md5').update(s).digest('hex');

const domains = new Map([['rvmail.local', { domain: 'rvmail.local', created: '2026-01-01' }]]);
const mailboxes = new Map([
  ['test1@rvmail.local', { username: 'test1@rvmail.local', full_name: 'Test 1', quota: '5 GB', active: 1 }],
  ['test2@rvmail.local', { username: 'test2@rvmail.local', full_name: 'Test 2', quota: '5 GB', active: 1 }],
]);

async function greenmailUser(email, password) {
  // Limitación: GreenMail no cambia la contraseña de un usuario que ya existe,
  // así que update_mailbox con contraseña nueva no se refleja en el IMAP de pruebas.
  await fetch(`${GREENMAIL}/api/user`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ email, login: email, password }),
  }).catch(() => undefined);
}

const handlers = {
  get_domains: () => ({ data: [...domains.values()].map((d) => ({ ...d, mailbox_count: [...mailboxes.keys()].filter((m) => m.endsWith('@' + d.domain)).length })) }),
  add_domain_new: (p) => (domains.has(p.domain) ? { status: false, msg: 'El dominio ya existe' } : (domains.set(p.domain, { domain: p.domain, created: new Date().toISOString() }), { status: true, msg: 'ok' })),
  delete_domain: (p) => (domains.delete(p.domain), { status: true, msg: 'ok' }),
  get_mailboxs: (p) => ({ data: [...mailboxes.values()].filter((m) => m.username.endsWith('@' + p.domain)) }),
  add_mailbox: async (p) => {
    if (mailboxes.has(p.username)) return { status: false, msg: 'El buzón ya existe' };
    mailboxes.set(p.username, { username: p.username, full_name: p.full_name, quota: p.quota, active: 1 });
    await greenmailUser(p.username, p.password);
    return { status: true, msg: 'ok' };
  },
  update_mailbox: async (p) => {
    const m = mailboxes.get(p.username);
    if (!m) return { status: false, msg: 'No existe' };
    Object.assign(m, { full_name: p.full_name, quota: p.quota, active: Number(p.active) });
    if (p.password) await greenmailUser(p.username, p.password);
    return { status: true, msg: 'ok' };
  },
  delete_mailbox: (p) => (mailboxes.delete(p.username), { status: true, msg: 'ok' }),
};

http
  .createServer((req, res) => {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', async () => {
      const url = new URL(req.url, 'http://x');
      const p = Object.fromEntries(new URLSearchParams(body));
      const send = (o) => res.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify(o));
      if (p.request_token !== md5(p.request_time + md5(KEY))) return send({ status: false, msg: 'Firma de la API no válida' });
      const fn = handlers[url.searchParams.get('s')];
      if (url.pathname !== '/plugin' || url.searchParams.get('name') !== 'mail_sys' || !fn) return send({ status: false, msg: 'Función no encontrada' });
      console.log(url.searchParams.get('s'), JSON.stringify({ ...p, password: p.password ? '***' : undefined, request_token: undefined }));
      send(await fn(p));
    });
  })
  .listen(PORT, () => console.log(`fake aaPanel en :${PORT} (clave "${KEY}")`));
