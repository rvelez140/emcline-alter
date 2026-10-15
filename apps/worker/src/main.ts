import { createDb, mailAccounts } from '@rvmail/db';
import { workerControlChannel, WorkerControl } from '@rvmail/shared';
import { and, eq, ne } from 'drizzle-orm';
import Redis from 'ioredis';
import { writeFile } from 'node:fs/promises';
import { hostname, tmpdir } from 'node:os';
import path from 'node:path';
import pino from 'pino';
import { AccountSession, SessionOptions } from './account-session';
import { LeaseManager } from './lease';

const log = pino({ level: process.env.LOG_LEVEL ?? 'info', base: { svc: 'worker' } });

const opts: SessionOptions = {
  syncDays: Number(process.env.SYNC_DAYS ?? 90),
  syncIntervalMs: Number(process.env.SYNC_INTERVAL_SEC ?? 300) * 1000,
  batchSize: Number(process.env.SYNC_BATCH_SIZE ?? 100),
};
const MAX_ACCOUNTS = Number(process.env.WORKER_MAX_ACCOUNTS ?? 500);
const SUPERVISE_MS = 30_000;
const RENEW_MS = 20_000;
/** Archivo de latido que lee el HEALTHCHECK del contenedor (ver Dockerfile). */
const HEARTBEAT_FILE = process.env.WORKER_HEARTBEAT_FILE ?? path.join(tmpdir(), 'rvmail-worker.heartbeat');

const { db, sql } = createDb(process.env.DATABASE_URL, 5);
const redisUrl = process.env.REDIS_URL ?? 'redis://localhost:6379';
const redis = new Redis(redisUrl);
const control = new Redis(redisUrl);
const owner = `${hostname()}:${process.pid}`;
const leases = new LeaseManager(redis, owner);
const sessions = new Map<string, AccountSession>();

async function startAccount(accountId: string) {
  if (sessions.has(accountId) || sessions.size >= MAX_ACCOUNTS) return;
  if (!(await leases.acquire(accountId))) return; // otro worker la tiene
  const account = await db.query.mailAccounts.findFirst({ where: eq(mailAccounts.id, accountId) });
  if (!account || !account.enabled) {
    await leases.release(accountId);
    return;
  }
  const session = new AccountSession(account, db, redis, log.child({ account: account.email }), opts);
  sessions.set(accountId, session);
  log.info({ account: account.email }, 'cuenta asignada a este worker');
  session.start().catch((e) => log.error({ err: e.message, account: account.email }, 'error iniciando sesión'));
}

async function stopAccount(accountId: string) {
  const s = sessions.get(accountId);
  if (!s) return;
  sessions.delete(accountId);
  await s.stop();
  await leases.release(accountId);
}

/** Toma cuentas libres y suelta las que ya no corresponden. */
async function supervise() {
  const rows = await db
    .select({ id: mailAccounts.id })
    .from(mailAccounts)
    .where(and(eq(mailAccounts.enabled, true), ne(mailAccounts.status, 'auth_error')));
  const wanted = new Set(rows.map((r) => r.id));
  for (const id of sessions.keys()) if (!wanted.has(id)) await stopAccount(id);
  for (const id of wanted) await startAccount(id);
  await writeFile(HEARTBEAT_FILE, String(Date.now())).catch(() => undefined);
}

async function renewLeases() {
  for (const id of sessions.keys()) {
    if (!(await leases.renew(id))) {
      log.warn({ accountId: id }, 'lease perdido, soltando cuenta');
      const s = sessions.get(id);
      sessions.delete(id);
      await s?.stop();
    }
  }
}

async function main() {
  log.info({ owner, ...opts }, 'worker iniciado');

  await control.subscribe(workerControlChannel);
  control.on('message', (_ch, payload) => {
    let msg: WorkerControl;
    try {
      msg = JSON.parse(payload);
    } catch {
      return;
    }
    if (msg.type === 'account.sync') {
      sessions.get(msg.accountId)?.requestSync();
      return;
    }
    void (async () => {
      // Cambió la cuenta: reiniciar la sesión (si la tenía este worker) o intentar tomarla
      await stopAccount(msg.accountId);
      if (msg.type === 'account.changed') await startAccount(msg.accountId);
    })().catch((e) => log.error({ err: e.message }, 'error procesando control'));
  });

  await supervise();
  setInterval(() => void supervise().catch((e) => log.error({ err: e.message }, 'supervise')), SUPERVISE_MS);
  setInterval(() => void renewLeases().catch((e) => log.error({ err: e.message }, 'renew')), RENEW_MS);
}

async function shutdown(signal: string) {
  log.info({ signal }, 'cerrando worker');
  await Promise.allSettled([...sessions.keys()].map(stopAccount));
  control.disconnect();
  redis.disconnect();
  await sql.end({ timeout: 5 });
  process.exit(0);
}

process.on('SIGTERM', () => void shutdown('SIGTERM'));
process.on('SIGINT', () => void shutdown('SIGINT'));

main().catch((e) => {
  log.fatal({ err: e.message }, 'error fatal');
  process.exit(1);
});
