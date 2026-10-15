import {
  BadRequestException,
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
  UseGuards,
} from '@nestjs/common';
import { Db, MailAccount, mailAccounts } from '@rvmail/db';
import { encryptSecret, workerControlChannel, WorkerControl } from '@rvmail/shared';
import { and, asc, eq } from 'drizzle-orm';
import Redis from 'ioredis';
import { z } from 'zod';
import { AuthGuard, CurrentUser, SessionUser } from '../auth/auth.guard';
import { parse } from '../common/zod';
import { config } from '../config';
import { DB, REDIS } from '../infra/infra.module';
import { ConnectionSettings, humanizeMailError, settingsFromAccount, smtpTransport, withImap } from '../mail/mail-clients';

const connectionSchema = z.object({
  imapHost: z.string().trim().min(1),
  imapPort: z.coerce.number().int().min(1).max(65535).default(993),
  imapSecure: z.boolean().default(true),
  smtpHost: z.string().trim().min(1),
  smtpPort: z.coerce.number().int().min(1).max(65535).default(465),
  smtpSecure: z.boolean().default(true),
  username: z.string().trim().min(1),
  password: z.string().min(1),
});

const createSchema = connectionSchema.extend({
  displayName: z.string().trim().min(1).max(100),
  email: z.string().trim().toLowerCase().email(),
  color: z.string().regex(/^#[0-9a-fA-F]{6}$/).optional(),
});

const updateSchema = createSchema.partial().extend({
  enabled: z.boolean().optional(),
  signatureHtml: z.string().max(20_000).nullable().optional(),
  sortOrder: z.number().int().optional(),
});

/** Nunca devolver la credencial cifrada al cliente. */
function toDto(a: MailAccount) {
  const { secretEnc: _secret, ...rest } = a;
  return rest;
}

@Controller('accounts')
@UseGuards(AuthGuard)
export class AccountsController {
  constructor(
    @Inject(DB) private readonly db: Db,
    @Inject(REDIS) private readonly redis: Redis,
  ) {}

  @Get()
  async list(@CurrentUser() user: SessionUser) {
    const rows = await this.db
      .select()
      .from(mailAccounts)
      .where(eq(mailAccounts.userId, user.id))
      .orderBy(asc(mailAccounts.sortOrder), asc(mailAccounts.createdAt));
    return rows.map(toDto);
  }

  /** Prueba IMAP y SMTP sin guardar nada. */
  @Post('test')
  @HttpCode(200)
  async test(@Body() body: unknown) {
    return this.testConnection(parse(connectionSchema, body));
  }

  @Post()
  async create(@CurrentUser() user: SessionUser, @Body() body: unknown) {
    const input = parse(createSchema, body);
    const result = await this.testConnection(input);
    if (!result.imap.ok || !result.smtp.ok) {
      throw new BadRequestException(
        `No se pudo conectar — IMAP: ${result.imap.error ?? 'OK'} · SMTP: ${result.smtp.error ?? 'OK'}`,
      );
    }
    const { password, ...rest } = input;
    const [row] = await this.db
      .insert(mailAccounts)
      .values({
        ...rest,
        userId: user.id,
        secretEnc: encryptSecret(password),
        source: config.mailserver.hostname && rest.imapHost === config.mailserver.hostname ? 'mailserver' : 'external',
      })
      .returning();
    await this.notifyWorker({ type: 'account.changed', accountId: row.id });
    return toDto(row);
  }

  @Patch(':id')
  async update(
    @CurrentUser() user: SessionUser,
    @Param('id', ParseUUIDPipe) id: string,
    @Body() body: unknown,
  ) {
    const current = await this.getOwned(user.id, id);
    const input = parse(updateSchema, body);
    const { password, ...rest } = input;
    const patch: Partial<MailAccount> = { ...rest };

    const connectionChanged =
      password !== undefined ||
      (['imapHost', 'imapPort', 'imapSecure', 'smtpHost', 'smtpPort', 'smtpSecure', 'username'] as const).some(
        (k) => rest[k] !== undefined,
      );
    if (connectionChanged) {
      const merged: ConnectionSettings = {
        ...settingsFromAccount(current),
        ...rest,
        ...(password ? { password } : {}),
      } as ConnectionSettings;
      const result = await this.testConnection(merged);
      if (!result.imap.ok || !result.smtp.ok) {
        throw new BadRequestException(
          `No se pudo conectar — IMAP: ${result.imap.error ?? 'OK'} · SMTP: ${result.smtp.error ?? 'OK'}`,
        );
      }
      if (password) patch.secretEnc = encryptSecret(password);
      patch.status = 'pending';
      patch.lastError = null;
    }

    const [row] = await this.db.update(mailAccounts).set(patch).where(eq(mailAccounts.id, id)).returning();
    await this.notifyWorker({ type: 'account.changed', accountId: id });
    return toDto(row);
  }

  @Delete(':id')
  @HttpCode(204)
  async remove(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string) {
    await this.getOwned(user.id, id);
    await this.db.delete(mailAccounts).where(eq(mailAccounts.id, id));
    await this.notifyWorker({ type: 'account.deleted', accountId: id });
  }

  private async getOwned(userId: string, id: string): Promise<MailAccount> {
    const row = await this.db.query.mailAccounts.findFirst({
      where: and(eq(mailAccounts.id, id), eq(mailAccounts.userId, userId)),
    });
    if (!row) throw new NotFoundException('Cuenta no encontrada');
    return row;
  }

  private async notifyWorker(msg: WorkerControl) {
    await this.redis.publish(workerControlChannel, JSON.stringify(msg));
  }

  private async testConnection(s: ConnectionSettings) {
    const [imap, smtp] = await Promise.all([
      withImap(s, async (c) => {
        await c.list();
      })
        .then(() => ({ ok: true as const, error: null }))
        .catch((e) => ({ ok: false as const, error: humanizeMailError(e) })),
      smtpTransport(s)
        .verify()
        .then(() => ({ ok: true as const, error: null }))
        .catch((e) => ({ ok: false as const, error: humanizeMailError(e) })),
    ]);
    return { imap, smtp };
  }
}
