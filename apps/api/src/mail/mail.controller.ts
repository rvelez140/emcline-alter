import {
  BadGatewayException,
  Body,
  Controller,
  Delete,
  Get,
  HttpCode,
  Inject,
  NotFoundException,
  Param,
  ParseUUIDPipe,
  Patch,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { Db, folders, mailAccounts, messages } from '@rvmail/db';
import { workerControlChannel, WorkerControl } from '@rvmail/shared';
import { and, asc, desc, eq, inArray, lt, or, sql, SQL } from 'drizzle-orm';
import Redis from 'ioredis';
import { simpleParser } from 'mailparser';
import { z } from 'zod';
import { AuthGuard, CurrentUser, SessionUser } from '../auth/auth.guard';
import { parse } from '../common/zod';
import { DB, REDIS } from '../infra/infra.module';
import { humanizeMailError, settingsFromAccount, smtpTransport, withImap } from './mail-clients';

const listSchema = z.object({
  accountId: z.string().uuid().optional(),
  folderId: z.string().uuid().optional(),
  /** Carpeta especial para la vista unificada: inbox, sent, drafts, trash, junk, archive */
  special: z.enum(['inbox', 'sent', 'drafts', 'trash', 'junk', 'archive']).default('inbox'),
  q: z.string().trim().max(200).optional(),
  unread: z.enum(['true', 'false']).optional().transform((v) => v === 'true'),
  /** Cursor = "<ISO date>|<id>" del último elemento de la página anterior */
  cursor: z.string().optional(),
  limit: z.coerce.number().int().min(1).max(200).default(50),
});

const patchSchema = z.object({
  seen: z.boolean().optional(),
  flagged: z.boolean().optional(),
});

const addressList = z.array(z.string().trim().email()).default([]);
const sendSchema = z.object({
  accountId: z.string().uuid(),
  to: addressList.refine((a) => a.length > 0, 'al menos un destinatario'),
  cc: addressList,
  bcc: addressList,
  subject: z.string().max(998).default(''),
  html: z.string().max(5_000_000).default(''),
  text: z.string().max(5_000_000).optional(),
  inReplyToMessageId: z.string().uuid().optional(),
});

const SPECIAL_USE: Record<string, string> = {
  inbox: '\\Inbox',
  sent: '\\Sent',
  drafts: '\\Drafts',
  trash: '\\Trash',
  junk: '\\Junk',
  archive: '\\Archive',
};

const BODY_CACHE_TTL = 3600;

@Controller()
@UseGuards(AuthGuard)
export class MailController {
  constructor(
    @Inject(DB) private readonly db: Db,
    @Inject(REDIS) private readonly redis: Redis,
  ) {}

  @Get('folders')
  async folders(@CurrentUser() user: SessionUser, @Query('accountId') accountId?: string) {
    const conds: SQL[] = [eq(mailAccounts.userId, user.id)];
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
  @Get('messages')
  async list(@CurrentUser() user: SessionUser, @Query() query: unknown) {
    const q = parse(listSchema, query);
    const conds: SQL[] = [eq(mailAccounts.userId, user.id), eq(mailAccounts.enabled, true)];
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

  /** Cuerpo completo: se descarga por IMAP bajo demanda y se cachea en Redis. */
  @Get('messages/:id')
  async get(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string) {
    const { msg, folder, account } = await this.getOwned(user.id, id);
    const cacheKey = `rvmail:body:${id}`;
    const cached = await this.redis.get(cacheKey);
    let body: { html: string | null; text: string | null; attachments: Array<{ filename: string | null; contentType: string; size: number }> };

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

    if (!msg.seen) await this.setFlags(user.id, id, { seen: true });
    return { ...msg, seen: true, accountColor: account.color, accountEmail: account.email, ...body };
  }

  @Patch('messages/:id')
  async patch(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string, @Body() body: unknown) {
    return this.setFlags(user.id, id, parse(patchSchema, body));
  }

  /** Mueve a la papelera (o borra definitivamente si ya está en la papelera). */
  @Delete('messages/:id')
  @HttpCode(204)
  async remove(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string) {
    const { msg, folder, account } = await this.getOwned(user.id, id);
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

  @Post('send')
  @HttpCode(200)
  async send(@CurrentUser() user: SessionUser, @Body() body: unknown) {
    const input = parse(sendSchema, body);
    const account = await this.db.query.mailAccounts.findFirst({
      where: and(eq(mailAccounts.id, input.accountId), eq(mailAccounts.userId, user.id)),
    });
    if (!account) throw new NotFoundException('Cuenta no encontrada');
    const settings = settingsFromAccount(account);

    let replyHeaders: { inReplyTo?: string; references?: string } = {};
    if (input.inReplyToMessageId) {
      const orig = await this.db.query.messages.findFirst({
        where: and(eq(messages.id, input.inReplyToMessageId), eq(messages.accountId, account.id)),
      });
      if (orig?.messageId) replyHeaders = { inReplyTo: orig.messageId, references: orig.messageId };
    }

    const html = account.signatureHtml ? `${input.html}<br><br>${account.signatureHtml}` : input.html;
    const mail = {
      from: { name: account.displayName, address: account.email },
      to: input.to,
      cc: input.cc,
      bcc: input.bcc,
      subject: input.subject,
      html,
      text: input.text,
      ...replyHeaders,
    };

    const transport = smtpTransport(settings);
    let info;
    try {
      info = await transport.sendMail(mail);
    } catch (e) {
      throw new BadGatewayException(`No se pudo enviar: ${humanizeMailError(e)}`);
    }

    // Guardar copia en "Enviados" (muchos servidores no lo hacen solos)
    const sent = await this.db.query.folders.findFirst({
      where: and(eq(folders.accountId, account.id), eq(folders.specialUse, '\\Sent')),
    });
    if (sent && process.env.APPEND_SENT !== 'false') {
      const MailComposer = (await import('nodemailer/lib/mail-composer')).default;
      const raw = await new MailComposer({ ...mail, bcc: undefined, messageId: info.messageId }).compile().build();
      await withImap(settings, (c) => c.append(sent.path, raw, ['\\Seen'])).catch(() => undefined);
    }
    await this.requestSync(account.id);
    return { messageId: info.messageId };
  }

  private async requestSync(accountId: string) {
    const msg: WorkerControl = { type: 'account.sync', accountId };
    await this.redis.publish(workerControlChannel, JSON.stringify(msg));
  }

  private async getOwned(userId: string, id: string) {
    const [row] = await this.db
      .select({ msg: messages, folder: folders, account: mailAccounts })
      .from(messages)
      .innerJoin(folders, eq(folders.id, messages.folderId))
      .innerJoin(mailAccounts, eq(mailAccounts.id, messages.accountId))
      .where(and(eq(messages.id, id), eq(mailAccounts.userId, userId)));
    if (!row) throw new NotFoundException('Mensaje no encontrado');
    return row;
  }

  private async setFlags(userId: string, id: string, patch: { seen?: boolean; flagged?: boolean }) {
    const { msg, folder, account } = await this.getOwned(userId, id);
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
        .where(inArray(folders.id, [folder.id]));
    }
    return updated;
  }
}
