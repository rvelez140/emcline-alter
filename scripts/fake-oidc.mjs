#!/usr/bin/env node
// Proveedor OpenID Connect FALSO para desarrollo y pruebas (simula Google).
// Aprueba al instante y emite id_tokens sin firmar con iss de Google.
//   node scripts/fake-oidc.mjs [puerto] [correo] [email_verified]
// En .env.dev: SSO_GOOGLE_CLIENT_ID=dev, SSO_GOOGLE_CLIENT_SECRET=dev,
//   RVMAIL_DEV_OIDC_BASE=http://localhost:7900  (solo binarios de desarrollo)
import http from 'node:http';
import { randomUUID } from 'node:crypto';

const PORT = Number(process.argv[2] ?? 7900);
let EMAIL = process.argv[3] ?? 'test1@rvmail.local';
let VERIFIED = (process.argv[4] ?? 'true') === 'true';
// Token de acceso fijo (GreenMail acepta como token XOAUTH2 la contraseña del buzón)
let ACCESS = null;
const codes = new Map();
const b64 = (o) => Buffer.from(JSON.stringify(o)).toString('base64url');

http
  .createServer((req, res) => {
    const url = new URL(req.url, 'http://x');
    if (url.pathname === '/config') {
      EMAIL = url.searchParams.get('email') ?? EMAIL;
      VERIFIED = (url.searchParams.get('verified') ?? String(VERIFIED)) === 'true';
      ACCESS = url.searchParams.get('at') ?? ACCESS;
      return res.end('ok');
    }
    if (url.pathname === '/auth') {
      const code = randomUUID();
      codes.set(code, { nonce: url.searchParams.get('nonce'), challenge: url.searchParams.get('code_challenge'), scope: url.searchParams.get('scope') });
      const to = new URL(url.searchParams.get('redirect_uri'));
      to.searchParams.set('code', code);
      to.searchParams.set('state', url.searchParams.get('state'));
      res.writeHead(302, { Location: to.toString() });
      return res.end();
    }
    if (url.pathname === '/token') {
      let body = '';
      req.on('data', (c) => (body += c));
      req.on('end', () => {
        const p = Object.fromEntries(new URLSearchParams(body));
        const send = (code, o) => res.writeHead(code, { 'Content-Type': 'application/json' }).end(JSON.stringify(o));
        if (p.client_id !== 'dev' || p.client_secret !== 'dev') return send(401, { error: 'invalid_client' });
        if (p.grant_type === 'refresh_token') {
          if (p.refresh_token === 'revoked') return send(400, { error: 'invalid_grant' });
          console.log('refresh', p.refresh_token);
          return send(200, { access_token: ACCESS ?? 'at-' + randomUUID(), expires_in: 3600 });
        }
        const c = codes.get(p.code);
        codes.delete(p.code);
        if (!c) return send(400, { error: 'invalid_grant' });
        if (c.challenge && !p.code_verifier) return send(400, { error: 'invalid_request', error_description: 'falta PKCE' });
        const now = Math.floor(Date.now() / 1000);
        const id = { iss: 'https://accounts.google.com', aud: 'dev', exp: now + 600, iat: now, nonce: c.nonce, sub: 'sub-' + EMAIL, email: EMAIL, email_verified: VERIFIED, name: 'Usuario Google' };
        console.log('token', c.scope, EMAIL);
        send(200, { access_token: ACCESS ?? 'at-' + randomUUID(), refresh_token: 'rt-' + randomUUID(), expires_in: 3600, id_token: `${b64({ alg: 'none' })}.${b64(id)}.` });
      });
      return;
    }
    res.writeHead(404).end();
  })
  .listen(PORT, () => console.log(`fake OIDC en :${PORT}`));
