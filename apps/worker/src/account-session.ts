import { Address, Db, Folder, folders, MailAccount, mailAccounts, messages } from '@rvmail/db';
import { AccountStatus, decryptSecret, eventsChannel, RvEvent } from '@rvmail/shared';
import { and, eq, gte, inArray, notInArray } from 'drizzle-orm';
import { FetchMessageObject, ImapFlow, MessageAddressObject } from 'imapflow';
import Redis from 'ioredis';
import { simpleParser } from 'mailparser';
import type { Logger } from 'pino';
import { folderPriority, resolveSpecialUse, SKIP_SPECIAL } from './special-use';

export type SessionOptions = {
  syncDays: number;
  syncIntervalMs: number;
  batchSize: number;
};

const MAX_BACKOFF_MS = 5 * 60_000;

/**
 * Una sesión IMAP persistente por cuenta:
 *  1. sincroniza carpetas y encabezados recientes,
 *  2. se queda en IDLE sobre INBOX (notificación inmediata de correo nuevo),
 *  3. repasa periódicamente las demás carpetas.
 * Todas las operaciones IMAP pasan por una cola para no pisarse.
 */
export class AccountSession {
  private client: ImapFlow | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private stopped = false;
  private backoffMs = 5_000;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private periodicTimer: NodeJS.Timeout | null = null;
  private inbox: Folder | null = null;

  constructor(
    private account: MailAccount,
    private readonly db: Db,
    private readonly redis: Redis,
    private readonly log: Logger,
    private readonly opts: SessionOptions,
  ) {}

  get id() {
    return this.account.id;
  }

  async start() {
    this.stopped = false;
    await this.connect();
  }

  async stop() {
    this.stopped = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    if (this.periodicTimer) clearInterval(this.periodicTimer);
    const c = this.client;
    this.client = null;
    if (c) await c.logout().catch(() => c.close());
  }

  /** Sincronización inmediata (la pide la API tras enviar, mover o borrar). */
  requestSync() {
    if (!this.client?.usable) return;
    void this.enqueue(() => this.fullSync()).catch((e) => this.log.warn({ err: e.message }, 'sync solicitado'));
  }

  // ---------------------------------------------------------------- conexión

  private async connect() {
    if (this.stopped) return;
    const fresh = await this.db.query.mailAccounts.findFirst({ where: eq(mailAccounts.id, this.account.id) });
    if (!fresh || !fresh.enabled) return;
    this.account = fresh;

    const client = new ImapFlow({
      host: fresh.imapHost,
      port: fresh.imapPort,
      secure: fresh.imapSecure,
      auth: { user: fresh.username, pass: decryptSecret(fresh.secretEnc) },
      logger: false,
      emitLogs: false,
      maxIdleTime: 5 * 60_000,
      tls: { rejectUnauthorized: process.env.IMAP_ALLOW_SELF_SIGNED !== 'true' },
    });
    this.client = client;

    client.on('error', (err) => this.log.warn({ err: err.message }, 'error IMAP'));
    client.on('close', () => {
      if (this.client === client) this.scheduleReconnect();
    });
    client.on('exists', (d: { path: string }) => this.enqueue(() => this.onExists(d.path)));
    client.on('flags', (d: { path: string; uid?: number; flags: Set<string> }) =>
      this.enqueue(() => this.onFlags(d.path, d.uid, d.flags)),
    );
    client.on('expunge', (d: { path: string }) => this.enqueue(() => this.onExpunge(d.path)));

    try {
      await this.setStatus('syncing');
      await client.connect();
      this.backoffMs = 5_000;
      await this.enqueue(() => this.fullSync());
      await this.setStatus('idle');
      if (this.periodicTimer) clearInterval(this.periodicTimer);
      this.periodicTimer = setInterval(() => {
        void this.enqueue(() => this.fullSync()).catch((e) => this.log.warn({ err: e.message }, 'sync periódico'));
      }, this.opts.syncIntervalMs);
    } catch (err) {
      const e = err as { authenticationFailed?: boolean; message?: string };
      if (e.authenticationFailed) {
        // No reintentar: el usuario debe corregir la contraseña
        await this.setStatus('auth_error', 'El servidor rechazó usuario o contraseña');
        this.stopped = true;
        await client.logout().catch(() => client.close());
        return;
      }
      await this.setStatus('error', e.message ?? 'Error de conexión');
      this.log.warn({ err: e.message }, 'fallo de conexión');
      client.close();
    }
  }

  private scheduleReconnect() {
    if (this.stopped || this.reconnectTimer) return;
    if (this.periodicTimer) clearInterval(this.periodicTimer);
    const wait = this.backoffMs;
    this.backoffMs = Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
    this.log.info({ waitMs: wait }, 'reconectando');
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.connect();
    }, wait);
  }

  private enqueue<T>(fn: () => Promise<T>): Promise<T> {
    const next = this.queue.then(fn, fn);
    this.queue = next.catch((e) => this.log.warn({ err: (e as Error).message }, 'operación IMAP falló'));
    return next;
  }

  // ---------------------------------------------------------------- sincronización

  private async fullSync() {
    const c = this.client;
    if (!c || !c.usable) return;
    const synced = await this.syncFolderList(c);
    for (const folder of synced) {
      if (this.stopped || !c.usable) return;
      await this.syncFolder(c, folder);
    }
    await this.db.update(mailAccounts).set({ lastSyncAt: new Date() }).where(eq(mailAccounts.id, this.account.id));
    // Volver a INBOX para que imapflow entre en IDLE allí
    if (this.inbox && c.usable) await c.mailboxOpen(this.inbox.path);
  }

  private async syncFolderList(c: ImapFlow): Promise<Folder[]> {
    const list = await c.list();
    const result: Folder[] = [];
    for (const box of list) {
      if (box.flags.has('\\Noselect') || box.flags.has('\\NonExistent')) continue;
      const specialUse = resolveSpecialUse(box.path, box.name, box.specialUse);
      if (specialUse && SKIP_SPECIAL.has(specialUse)) continue;
      const [row] = await this.db
        .insert(folders)
        .values({
          accountId: this.account.id,
          path: box.path,
          name: box.name,
          delimiter: box.delimiter,
          specialUse,
          subscribed: box.subscribed ?? true,
        })
        .onConflictDoUpdate({
          target: [folders.accountId, folders.path],
          set: { name: box.name, delimiter: box.delimiter, specialUse },
        })
        .returning();
      result.push(row);
    }
    // Carpetas que ya no existen en el servidor
    const paths = result.map((f) => f.path);
    if (paths.length) {
      await this.db
        .delete(folders)
        .where(and(eq(folders.accountId, this.account.id), notInArray(folders.path, paths)));
    }
    this.inbox = result.find((f) => f.specialUse === '\\Inbox') ?? null;
    return result.sort((a, b) => folderPriority(a.specialUse) - folderPriority(b.specialUse));
  }

  private async syncFolder(c: ImapFlow, folder: Folder): Promise<number> {
    const lock = await c.getMailboxLock(folder.path, { readOnly: true });
    let inserted = 0;
    try {
      const mb = c.mailbox;
      if (!mb) return 0;
      const uidValidity = Number(mb.uidValidity);
      let lastUid = folder.lastUid;

      if (folder.uidValidity !== null && folder.uidValidity !== uidValidity) {
        // UIDVALIDITY cambió: los UID antiguos ya no sirven
        this.log.info({ folder: folder.path }, 'UIDVALIDITY cambió, resincronizando carpeta');
        await this.db.delete(messages).where(eq(messages.folderId, folder.id));
        lastUid = 0;
      }

      let uids: number[] = [];
      if (mb.exists > 0) {
        if (lastUid === 0) {
          const since = new Date(Date.now() - this.opts.syncDays * 86_400_000);
          uids = ((await c.search({ since }, { uid: true })) || []) as number[];
        } else if (mb.uidNext > lastUid + 1) {
          uids = ((await c.search({ uid: `${lastUid + 1}:*` }, { uid: true })) || []) as number[];
          uids = uids.filter((u) => u > lastUid);
        }
      }

      uids.sort((a, b) => b - a); // más recientes primero
      for (let i = 0; i < uids.length; i += this.opts.batchSize) {
        if (this.stopped) break;
        const batch = uids.slice(i, i + this.opts.batchSize);
        inserted += await this.fetchAndStore(c, folder, batch);
      }
      const maxUid = uids.length ? Math.max(...uids) : lastUid;

      await this.db
        .update(folders)
        .set({
          uidValidity,
          lastUid: Math.max(maxUid, lastUid),
          highestModseq: mb.highestModseq ? String(mb.highestModseq) : null,
          syncedAt: new Date(),
        })
        .where(eq(folders.id, folder.id));
    } finally {
      lock.release();
    }

    await this.refreshCounts(c, folder);
    if (inserted > 0) {
      await this.publish({ type: 'message.new', accountId: this.account.id, folderId: folder.id, count: inserted });
    }
    return inserted;
  }

  private async fetchAndStore(c: ImapFlow, folder: Folder, uids: number[]): Promise<number> {
    const rows: Array<typeof messages.$inferInsert> = [];
    const range = uids.join(',');
    for await (const m of c.fetch(
      range,
      { uid: true, envelope: true, flags: true, size: true, internalDate: true, bodyStructure: true, source: { maxLength: 16_384 } },
      { uid: true },
    )) {
      rows.push(await this.toRow(folder, m));
    }
    if (!rows.length) return 0;
    const res = await this.db.insert(messages).values(rows).onConflictDoNothing().returning({ id: messages.id });
    return res.length;
  }

  private async toRow(folder: Folder, m: FetchMessageObject): Promise<typeof messages.$inferInsert> {
    const env = m.envelope;
    const flags = [...(m.flags ?? new Set<string>())];
    let snippet = '';
    if (m.source) {
      try {
        const parsed = await simpleParser(m.source, { skipImageLinks: true, skipTextToHtml: true });
        const text = parsed.text || (parsed.html ? String(parsed.html).replace(/<[^>]+>/g, ' ') : '');
        snippet = text.replace(/\s+/g, ' ').trim().slice(0, 240);
      } catch {
        /* cuerpo truncado o mal formado: sin vista previa */
      }
    }
    const from = env?.from?.[0];
    const internal = m.internalDate ? new Date(m.internalDate) : null;
    return {
      accountId: this.account.id,
      folderId: folder.id,
      uid: m.uid,
      messageId: env?.messageId ?? null,
      inReplyTo: env?.inReplyTo ?? null,
      threadKey: normalizeSubject(env?.subject ?? ''),
      subject: env?.subject ?? '',
      fromName: from?.name || null,
      fromAddress: from?.address || null,
      to: toAddresses(env?.to),
      cc: toAddresses(env?.cc),
      date: env?.date ? new Date(env.date) : (internal ?? new Date()),
      snippet,
      flags,
      seen: flags.includes('\\Seen'),
      flagged: flags.includes('\\Flagged'),
      hasAttachments: hasAttachment(m.bodyStructure),
      size: m.size ?? 0,
    };
  }

  private async refreshCounts(c: ImapFlow, folder: Folder) {
    try {
      const st = await c.status(folder.path, { messages: true, unseen: true });
      if (!st) return;
      await this.db
        .update(folders)
        .set({ totalCount: st.messages ?? 0, unreadCount: st.unseen ?? 0 })
        .where(eq(folders.id, folder.id));
    } catch {
      /* algunos servidores no permiten STATUS sobre la carpeta abierta */
    }
  }

  // ---------------------------------------------------------------- eventos IDLE

  private async onExists(path: string) {
    const c = this.client;
    const folder = await this.folderByPath(path);
    if (!c || !folder) return;
    await this.syncFolder(c, folder);
  }

  private async onFlags(path: string, uid: number | undefined, flagSet: Set<string>) {
    const folder = await this.folderByPath(path);
    if (!folder || !uid) return;
    const flags = [...flagSet];
    const updated = await this.db
      .update(messages)
      .set({ flags, seen: flags.includes('\\Seen'), flagged: flags.includes('\\Flagged') })
      .where(and(eq(messages.folderId, folder.id), eq(messages.uid, uid)))
      .returning({ id: messages.id });
    if (this.client) await this.refreshCounts(this.client, folder);
    if (updated.length) {
      await this.publish({ type: 'message.updated', accountId: this.account.id, messageIds: updated.map((u) => u.id) });
    }
  }

  /** EXPUNGE solo trae número de secuencia: se reconcilia la lista de UID. */
  private async onExpunge(path: string) {
    const c = this.client;
    const folder = await this.folderByPath(path);
    if (!c || !folder) return;
    const lock = await c.getMailboxLock(folder.path, { readOnly: true });
    try {
      const since = new Date(Date.now() - this.opts.syncDays * 86_400_000);
      const present = new Set(((await c.search({ since }, { uid: true })) || []) as number[]);
      const local = await this.db
        .select({ id: messages.id, uid: messages.uid })
        .from(messages)
        .where(and(eq(messages.folderId, folder.id), gte(messages.date, since)));
      const gone = local.filter((m) => !present.has(m.uid)).map((m) => m.id);
      if (gone.length) await this.db.delete(messages).where(inArray(messages.id, gone));
    } finally {
      lock.release();
    }
    await this.refreshCounts(c, folder);
    await this.publish({ type: 'folder.updated', accountId: this.account.id, folderId: folder.id });
  }

  // ---------------------------------------------------------------- utilidades

  private folderByPath(path: string) {
    return this.db.query.folders.findFirst({
      where: and(eq(folders.accountId, this.account.id), eq(folders.path, path)),
    });
  }

  private async setStatus(status: AccountStatus, error: string | null = null) {
    await this.db.update(mailAccounts).set({ status, lastError: error }).where(eq(mailAccounts.id, this.account.id));
    await this.publish({ type: 'account.status', accountId: this.account.id, status, error });
  }

  private async publish(event: RvEvent) {
    await this.redis.publish(eventsChannel(this.account.userId), JSON.stringify(event));
  }
}

function toAddresses(list?: MessageAddressObject[]): Address[] {
  return (list ?? []).filter((a) => a.address).map((a) => ({ name: a.name || null, address: a.address! }));
}

function normalizeSubject(s: string): string {
  return s.replace(/^\s*((re|fw|fwd|rv|res|enc)\s*:\s*)+/i, '').trim().toLowerCase();
}

type BodyNode = { disposition?: string; childNodes?: BodyNode[] };
function hasAttachment(node?: BodyNode): boolean {
  if (!node) return false;
  if (node.disposition?.toLowerCase() === 'attachment') return true;
  return (node.childNodes ?? []).some(hasAttachment);
}
