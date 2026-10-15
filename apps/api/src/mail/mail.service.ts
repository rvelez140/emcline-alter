import {
  BadGatewayException,
  HttpException,
  HttpStatus,
  Inject,
  Injectable,
  Logger,
  NotFoundException,
} from '@nestjs/common';
import { Db, folders, MailAccount, mailAccounts, messages } from '@rvmail/db';
import { workerControlChannel, WorkerControl } from '@rvmail/shared';
import { and, asc, desc, eq, inArray, lt, or, sql, SQL } from 'drizzle-orm';
import Redis from 'ioredis';
import { simpleParser } from 'mailparser';
import { z } from 'zod';
import { assertScope, SessionUser } from '../auth/auth.guard';
import { config } from '../config';
import { DB, REDIS } from '../infra/infra.module';
import { humanizeMailError, settingsFromAccount, smtpTransport, withImap } from './mail-clients';

export const SPECIALS = ['inbox', 'sent', 'drafts', 'trash', 'junk', 'archive'] as const;
export type Special = (typeof SPECIALS)[number];

const SPECIAL_USE: Record<Special, string> = {
  inbox: '\\Inbox',
  sent: '\\Sent',
  drafts: '\\Drafts',
  trash: '\\Trash',
  junk: '\\Junk',
  archive: '\\Archive',
};

export const listSchema = z.object({
  accountId: z.string().uuid().optional(),
  folderId: z.string().uuid().optional(),
  /** Carpeta especial para la vista unificada: inbox, sent, drafts, trash, junk, archive */
  special: z.enum(SPECIALS).default('inbox'),
  q: z.string().trim().max(200).optional(),
  unread: z
    .union([z.boolean(), z.enum(['true', 'false'])])
    .optional()
    .transform((v) => v === true || v === 'true'),
  /** Cursor = "<ISO date>|<id>" del último elemento de la página anterior */
  cursor: z.string().optional(),
  limit: z.coerce.number().int().min(1).max(200).default(50),
});
export type ListQuery = z.output<typeof listSchema>;

const addressList = z.array(z.string().trim().email()).default([]);
export const composeSchema = z.object({
  accountId: z.string().uuid(),
  to: addressList,
  cc: addressList,
  bcc: addressList,
  subject: z.string().max(998).default(''),
  html: z.string().max(5_000_000).default(''),
  text: z.string().max(5_000_000).optional(),
  inReplyToMessageId: z.string().uuid().optional(),
});
export const sendSchema = composeSchema.extend({
  to: addressList.refine((a) => a.length > 0, 'al menos un destinatario'),
});
export type ComposeInput = z.output<typeof composeSchema>;

export type MessageBody = {
  html: string | null;
  text: string | null;
  attachments: Array<{ filename: string | null; contentType: string; size: number }>;
};

const BODY_CACHE_TTL = 3600;

/**
 * Lógica de correo compartida por la API REST (navegador/apps) y el servidor
 * MCP (agentes de IA). Toda consulta se limita a las cuentas del usuario y,
 * si la petición llega con token, a las cuentas y ámbitos que el token permite.
 */
@Injectable()
export class MailService {
  private readonly log = new Logger('Mail');

  constructor(
    @Inject(DB) private readonly db: Db,
    @Inject(REDIS) private readonly redis: Redis,
  ) {}

  /** Condiciones SQL que restringen a las cuentas visibles para este usuario/token. */
  private accountScope(user: SessionUser): SQL[] {
    const conds: SQL[] = [eq(mailAccounts.userId, user.id)];
    const ids = user.token?.accountIds;
    if (ids) conds.push(ids.length ? inArray(mailAccounts.id, ids) : sql`false`);
    return conds;
  }

  async listAccounts(user: SessionUser) {
    return this.db
      .select({
        id: mailAccounts.id,
        displayName: mailAccounts.displayName,
        email: mailAccounts.email,
        status: mailAccounts.status,
        enabled: mailAccounts.enabled,
        source: mailAccounts.source,
      })
      .from(mailAccounts)
      .where(and(...this.accountScope(user)))
      .orderBy(asc(mailAccounts.sortOrder), asc(mailAccounts.createdAt));
  }

  async listFolders(user: SessionUser, accountId?: string) {
    const conds = this.accountScope(user);
    if (accountId) conds.push(eq(folders.accountId, accountId));
    return this.db
      .select({
        id: folders.id,
        accountId: folders.accountId,
        path: folders.path,
        name: folders.name,
        delimiter: folders.delimiter,
        specialUse: folders.specialUse,
        unreadCount: folders.unreadCount,
        totalCount: folders.totalCount,
      })
      .from(folders)
      .innerJoin(mailAccounts, eq(mailAccounts.id, folders.accountId))
      .where(and(...conds))
      .orderBy(asc(folders.accountId), asc(folders.path));
  }

  /**
   * Lista mensajes. Sin folderId → vista unificada de la carpeta especial
   * indicada (por defecto bandeja de entrada) de todas las cuentas o de accountId.
   */
  async listMessages(user: SessionUser, q: ListQuery) {
    const conds = [...this.accountScope(user), eq(mailAccounts.enabled, true)];
    if (q.accountId) conds.push(eq(messages.accountId, q.accountId));
    if (q.folderId) conds.push(eq(messages.folderId, q.folderId));
    else conds.push(eq(folders.specialUse, SPECIAL_USE[q.special]));
    if (q.unread) conds.push(eq(messages.seen, false));
    if (q.q) conds.push(sql`${messages.search} @@ websearch_to_tsquery('spanish', ${q.q})`);
    if (q.cursor) {
      const [d, id] = q.cursor.split('|');
      const date = new Date(d);
      if (!Number.isNaN(date.getTime()) && id) {
        conds.push(or(lt(messages.date, date), and(eq(messages.date, date), lt(messages.id, id)))!);
      }
    }

    const rows = await this.db
      .select({
        id: messages.id,
        accountId: messages.accountId,
        folderId: messages.folderId,
        subject: messages.subject,
        fromName: messages.fromName,
        fromAddress: messages.fromAddress,
        to: messages.to,
        date: messages.date,
        snippet: messages.snippet,
        seen: messages.seen,
        flagged: messages.flagged,
        hasAttachments: messages.hasAttachments,
        accountColor: mailAccounts.color,
        accountEmail: mailAccounts.email,
      })
      .from(messages)
      .innerJoin(folders, eq(folders.id, messages.folderId))
      .innerJoin(mailAccounts, eq(mailAccounts.id, messages.accountId))
      .where(and(...conds))
      .orderBy(desc(messages.date), desc(messages.id))
      .limit(q.limit + 1);

    const hasMore = rows.length > q.limit;
    const items = hasMore ? rows.slice(0, q.limit) : rows;
    const last = items[items.length - 1];
    return { items, nextCursor: hasMore && last ? `${last.date.toISOString()}|${last.id}` : null };
  }

  /**
   * Cuerpo completo: se descarga por IMAP bajo demanda y se cachea en Redis.
   * markSeen marca como leído (requiere mail:modify si es un token).
   */
  async getMessage(user: SessionUser, id: string, opts: { markSeen: boolean }) {
    const { msg, folder, account } = await this.getOwned(user, id);
    const cacheKey = `rvmail:body:${id}`;
    const cached = await this.redis.get(cacheKey);
    let body: MessageBody;

    if (cached) {
      body = JSON.parse(cached);
    } else {
      const source = await withImap(settingsFromAccount(account), async (c) => {
        const lock = await c.getMailboxLock(folder.path, { readOnly: true });
        try {
          const m = await c.fetchOne(String(msg.uid), { source: true }, { uid: true });
          if (!m || !m.source) throw new NotFoundException('El mensaje ya no existe en el servidor');
          return m.source;
        } finally {
          lock.release();
        }
      });
      const parsed = await simpleParser(source);
      body = {
        html: parsed.html || null,
        text: parsed.text ?? null,
        attachments: parsed.attachments.map((a) => ({
          filename: a.filename ?? null,
          contentType: a.contentType,
          size: a.size,
        })),
      };
      await this.redis.set(cacheKey, JSON.stringify(body), 'EX', BODY_CACHE_TTL);
    }

    let seen = msg.seen;
    if (!seen && opts.markSeen) {
      await this.setFlags(user, id, { seen: true });
      seen = true;
    }
    return { ...msg, seen, accountColor: account.color, accountEmail: account.email, ...body };
  }

  async setFlags(user: SessionUser, id: string, patch: { seen?: boolean; flagged?: boolean }) {
    assertScope(user, 'mail:modify');
    const { msg, folder, account } = await this.getOwned(user, id);
    await withImap(settingsFromAccount(account), async (c) => {
      const lock = await c.getMailboxLock(folder.path);
      try {
        const uid = String(msg.uid);
        if (patch.seen === true) await c.messageFlagsAdd(uid, ['\\Seen'], { uid: true });
        if (patch.seen === false) await c.messageFlagsRemove(uid, ['\\Seen'], { uid: true });
        if (patch.flagged === true) await c.messageFlagsAdd(uid, ['\\Flagged'], { uid: true });
        if (patch.flagged === false) await c.messageFlagsRemove(uid, ['\\Flagged'], { uid: true });
      } finally {
        lock.release();
      }
    });
    const [updated] = await this.db.update(messages).set(patch).where(eq(messages.id, id)).returning();
    if (patch.seen !== undefined && patch.seen !== msg.seen) {
      await this.db
        .update(folders)
        .set({ unreadCount: sql`greatest(${folders.unreadCount} + ${patch.seen ? -1 : 1}, 0)` })
        .where(eq(folders.id, folder.id));
    }
    return updated;
  }

  /** Mueve a la papelera (o borra definitivamente si ya está en la papelera). */
  async remove(user: SessionUser, id: string) {
    assertScope(user, 'mail:modify');
    const { msg, folder, account } = await this.getOwned(user, id);
    const trash = await this.db.query.folders.findFirst({
      where: and(eq(folders.accountId, account.id), eq(folders.specialUse, '\\Trash')),
    });
    await withImap(settingsFromAccount(account), async (c) => {
      const lock = await c.getMailboxLock(folder.path);
      try {
        if (trash && trash.id !== folder.id) await c.messageMove(String(msg.uid), trash.path, { uid: true });
        else await c.messageDelete(String(msg.uid), { uid: true });
      } finally {
        lock.release();
      }
    });
    await this.db.delete(messages).where(eq(messages.id, id));
    await this.requestSync(account.id);
  }

  async send(user: SessionUser, input: ComposeInput) {
    assertScope(user, 'mail:send');
    const account = await this.getAccount(user, input.accountId);
    if (user.token) await this.enforceAgentSendLimit(user.token.id);
    const mail = await this.buildMail(account, input);
    const settings = settingsFromAccount(account);

    let info;
    try {
      info = await smtpTransport(settings).sendMail(mail);
    } catch (e) {
      throw new BadGatewayException(`No se pudo enviar: ${humanizeMailError(e)}`);
    }
    if (user.token) {
      this.log.log(`agente "${user.token.name}" envió correo desde ${account.email} a ${mail.to.length} destinatario(s)`);
    }

    // Guardar copia en "Enviados" (muchos servidores no lo hacen solos)
    const sent = await this.db.query.folders.findFirst({
      where: and(eq(folders.accountId, account.id), eq(folders.specialUse, '\\Sent')),
    });
    if (sent && process.env.APPEND_SENT !== 'false') {
      const raw = await this.compile({ ...mail, bcc: undefined, messageId: info.messageId });
      await withImap(settings, (c) => c.append(sent.path, raw, ['\\Seen'])).catch(() => undefined);
    }
    await this.requestSync(account.id);
    return { messageId: info.messageId as string };
  }

  /**
   * Guarda un borrador en la carpeta "Borradores" del servidor. Es la vía
   * segura para agentes de IA: una persona lo revisa y lo envía desde RV Mail.
   */
  async saveDraft(user: SessionUser, input: ComposeInput) {
    assertScope(user, 'mail:draft');
    const account = await this.getAccount(user, input.accountId);
    const drafts = await this.db.query.folders.findFirst({
      where: and(eq(folders.accountId, account.id), eq(folders.specialUse, '\\Drafts')),
    });
    const path = drafts?.path ?? 'Drafts';
    const raw = await this.compile(await this.buildMail(account, input));
    try {
      await withImap(settingsFromAccount(account), async (c) => {
        // Si la cuenta aún no tiene carpeta de borradores, se crea
        if (!drafts) await c.mailboxCreate(path).catch(() => undefined);
        await c.append(path, raw, ['\\Draft', '\\Seen']);
      });
    } catch (e) {
      throw new BadGatewayException(`No se pudo guardar el borrador: ${humanizeMailError(e)}`);
    }
    await this.requestSync(account.id);
    return { folder: path };
  }

  private async buildMail(account: MailAccount, input: ComposeInput) {
    let replyHeaders: { inReplyTo?: string; references?: string } = {};
    if (input.inReplyToMessageId) {
      const orig = await this.db.query.messages.findFirst({
        where: and(eq(messages.id, input.inReplyToMessageId), eq(messages.accountId, account.id)),
      });
      if (orig?.messageId) replyHeaders = { inReplyTo: orig.messageId, references: orig.messageId };
    }
    const html = account.signatureHtml ? `${input.html}<br><br>${account.signatureHtml}` : input.html;
    return {
      from: { name: account.displayName, address: account.email },
      to: input.to,
      cc: input.cc,
      bcc: input.bcc,
      subject: input.subject,
      html,
      text: input.text,
      ...replyHeaders,
    };
  }

  private async compile(mail: Record<string, unknown>): Promise<Buffer> {
    const MailComposer = (await import('nodemailer/lib/mail-composer')).default;
    return new MailComposer(mail).compile().build();
  }

  /** Límite de envíos por hora para cada token: protege la reputación del dominio. */
  private async enforceAgentSendLimit(tokenId: string) {
    const key = `rvmail:rl:agent-send:${tokenId}`;
    const n = await this.redis.incr(key);
    if (n === 1) await this.redis.expire(key, 3600);
    if (n > config.agentSendLimitPerHour) {
      throw new HttpException(
        `Límite de ${config.agentSendLimitPerHour} envíos por hora alcanzado para este token`,
        HttpStatus.TOO_MANY_REQUESTS,
      );
    }
  }

  private async getAccount(user: SessionUser, accountId: string): Promise<MailAccount> {
    const [account] = await this.db
      .select()
      .from(mailAccounts)
      .where(and(eq(mailAccounts.id, accountId), ...this.accountScope(user)));
    if (!account) throw new NotFoundException('Cuenta no encontrada');
    return account;
  }

  private async getOwned(user: SessionUser, id: string) {
    const [row] = await this.db
      .select({ msg: messages, folder: folders, account: mailAccounts })
      .from(messages)
      .innerJoin(folders, eq(folders.id, messages.folderId))
      .innerJoin(mailAccounts, eq(mailAccounts.id, messages.accountId))
      .where(and(eq(messages.id, id), ...this.accountScope(user)));
    if (!row) throw new NotFoundException('Mensaje no encontrado');
    return row;
  }

  private async requestSync(accountId: string) {
    const msg: WorkerControl = { type: 'account.sync', accountId };
    await this.redis.publish(workerControlChannel, JSON.stringify(msg));
  }
}
