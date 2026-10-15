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
  Post,
  UseGuards,
} from '@nestjs/common';
import { API_SCOPES, apiTokens, Db, mailAccounts } from '@rvmail/db';
import { and, desc, eq, inArray, isNull } from 'drizzle-orm';
import { randomBytes } from 'node:crypto';
import { z } from 'zod';
import { AuthGuard, CurrentUser, hashToken, SessionUser, TOKEN_PREFIX } from '../auth/auth.guard';
import { parse } from '../common/zod';
import { DB } from '../infra/infra.module';

const createSchema = z.object({
  name: z.string().trim().min(1).max(80),
  scopes: z.array(z.enum(API_SCOPES)).min(1, 'elige al menos un permiso'),
  /** null/omitido = todas las cuentas */
  accountIds: z.array(z.string().uuid()).nullable().optional(),
  /** Días hasta que expire; null = no expira */
  expiresInDays: z.number().int().min(1).max(3650).nullable().optional(),
});

/**
 * Tokens de API para agentes de IA (MCP) e integraciones. Solo se gestionan
 * desde la sesión del navegador: un token no puede crear otros tokens.
 */
@Controller('tokens')
@UseGuards(AuthGuard)
export class TokensController {
  constructor(@Inject(DB) private readonly db: Db) {}

  @Get()
  async list(@CurrentUser() user: SessionUser) {
    return this.db
      .select({
        id: apiTokens.id,
        name: apiTokens.name,
        prefix: apiTokens.prefix,
        scopes: apiTokens.scopes,
        accountIds: apiTokens.accountIds,
        lastUsedAt: apiTokens.lastUsedAt,
        expiresAt: apiTokens.expiresAt,
        createdAt: apiTokens.createdAt,
      })
      .from(apiTokens)
      .where(and(eq(apiTokens.userId, user.id), isNull(apiTokens.revokedAt)))
      .orderBy(desc(apiTokens.createdAt));
  }

  /** Devuelve el token en claro UNA sola vez; después solo se conserva su hash. */
  @Post()
  async create(@CurrentUser() user: SessionUser, @Body() body: unknown) {
    const input = parse(createSchema, body);
    const accountIds = input.accountIds?.length ? [...new Set(input.accountIds)] : null;
    if (accountIds) {
      const owned = await this.db
        .select({ id: mailAccounts.id })
        .from(mailAccounts)
        .where(and(eq(mailAccounts.userId, user.id), inArray(mailAccounts.id, accountIds)));
      if (owned.length !== accountIds.length) throw new BadRequestException('Alguna de las cuentas no es tuya');
    }
    const token = `${TOKEN_PREFIX}${randomBytes(32).toString('base64url')}`;
    const [row] = await this.db
      .insert(apiTokens)
      .values({
        userId: user.id,
        name: input.name,
        tokenHash: hashToken(token),
        prefix: token.slice(0, TOKEN_PREFIX.length + 6),
        scopes: input.scopes,
        accountIds,
        expiresAt: input.expiresInDays ? new Date(Date.now() + input.expiresInDays * 86_400_000) : null,
      })
      .returning();
    return { id: row.id, name: row.name, prefix: row.prefix, scopes: row.scopes, token };
  }

  @Delete(':id')
  @HttpCode(204)
  async revoke(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string) {
    const [row] = await this.db
      .update(apiTokens)
      .set({ revokedAt: new Date() })
      .where(and(eq(apiTokens.id, id), eq(apiTokens.userId, user.id), isNull(apiTokens.revokedAt)))
      .returning({ id: apiTokens.id });
    if (!row) throw new NotFoundException('Token no encontrado');
  }
}
