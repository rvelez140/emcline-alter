import {
  CanActivate,
  createParamDecorator,
  ExecutionContext,
  ForbiddenException,
  Inject,
  Injectable,
  SetMetadata,
  UnauthorizedException,
} from '@nestjs/common';
import { Reflector } from '@nestjs/core';
import { ApiScope, apiTokens, Db, users } from '@rvmail/db';
import { and, eq, isNull } from 'drizzle-orm';
import type { Request } from 'express';
import jwt from 'jsonwebtoken';
import { createHash } from 'node:crypto';
import { config } from '../config';
import { DB } from '../infra/infra.module';

export const SESSION_COOKIE = 'rv_session';
/** Prefijo de los tokens de API: permite reconocerlos (y que los escáneres de secretos los detecten). */
export const TOKEN_PREFIX = 'rvm_';

export type TokenGrant = { id: string; name: string; scopes: ApiScope[]; accountIds: string[] | null };

export type SessionUser = {
  id: string;
  email: string;
  role: 'admin' | 'user';
  /** Presente solo si la petición llegó con un token de API (agente de IA / integración). */
  token?: TokenGrant;
};

export function signSession(user: SessionUser): string {
  return jwt.sign({ id: user.id, email: user.email, role: user.role }, config.jwtSecret, {
    expiresIn: `${config.sessionDays}d`,
  });
}

export function readSession(req: Request): SessionUser | null {
  const token = req.cookies?.[SESSION_COOKIE];
  if (!token) return null;
  try {
    const p = jwt.verify(token, config.jwtSecret) as SessionUser;
    return { id: p.id, email: p.email, role: p.role };
  } catch {
    return null;
  }
}

export function hashToken(token: string): string {
  return createHash('sha256').update(token).digest('hex');
}

/** Resuelve un token Bearer a su usuario y permisos. null si no existe, expiró o fue revocado. */
export async function resolveApiToken(db: Db, raw: string): Promise<SessionUser | null> {
  if (!raw.startsWith(TOKEN_PREFIX)) return null;
  const [hit] = await db
    .select({ token: apiTokens, user: users })
    .from(apiTokens)
    .innerJoin(users, eq(users.id, apiTokens.userId))
    .where(and(eq(apiTokens.tokenHash, hashToken(raw)), isNull(apiTokens.revokedAt)));
  if (!hit) return null;
  const row = { ...hit.token, user: hit.user };
  if (row.expiresAt && row.expiresAt.getTime() < Date.now()) return null;
  // Registrar último uso sin bloquear la petición (como mucho una escritura por minuto)
  if (!row.lastUsedAt || Date.now() - row.lastUsedAt.getTime() > 60_000) {
    void db.update(apiTokens).set({ lastUsedAt: new Date() }).where(eq(apiTokens.id, row.id)).catch(() => undefined);
  }
  return {
    id: row.user.id,
    email: row.user.email,
    role: row.user.role,
    token: { id: row.id, name: row.name, scopes: row.scopes, accountIds: row.accountIds ?? null },
  };
}

export function bearerFrom(req: Request): string | null {
  const h = req.headers.authorization;
  if (!h || !/^Bearer\s+/i.test(h)) return null;
  return h.replace(/^Bearer\s+/i, '').trim();
}

const SCOPES_KEY = 'rvmail:scopes';
/**
 * Marca un endpoint como accesible con token de API, exigiendo estos ámbitos.
 * Los endpoints sin este decorador solo aceptan la sesión del navegador.
 */
export const RequireScopes = (...scopes: ApiScope[]) => SetMetadata(SCOPES_KEY, scopes);

/** ¿El usuario (sesión o token) tiene el ámbito? Las sesiones del navegador lo tienen todo. */
export function hasScope(user: SessionUser, scope: ApiScope): boolean {
  return !user.token || user.token.scopes.includes(scope);
}

export function assertScope(user: SessionUser, scope: ApiScope): void {
  if (!hasScope(user, scope)) {
    throw new ForbiddenException(`Este token no tiene permiso "${scope}"`);
  }
}

@Injectable()
export class AuthGuard implements CanActivate {
  constructor(
    @Inject(DB) private readonly db: Db,
    private readonly reflector: Reflector,
  ) {}

  async canActivate(ctx: ExecutionContext): Promise<boolean> {
    const req = ctx.switchToHttp().getRequest<Request & { user?: SessionUser }>();
    const bearer = bearerFrom(req);
    if (bearer) {
      const user = await resolveApiToken(this.db, bearer);
      if (!user) throw new UnauthorizedException('Token no válido, expirado o revocado');
      const required = this.reflector.getAllAndOverride<ApiScope[] | undefined>(SCOPES_KEY, [
        ctx.getHandler(),
        ctx.getClass(),
      ]);
      if (!required) throw new ForbiddenException('Este endpoint no admite tokens de API');
      for (const s of required) assertScope(user, s);
      req.user = user;
      return true;
    }
    const user = readSession(req);
    if (!user) throw new UnauthorizedException('Sesión no válida o expirada');
    req.user = user;
    return true;
  }
}

/** Solo administradores de la instalación (nunca con token de API). */
@Injectable()
export class AdminGuard implements CanActivate {
  canActivate(ctx: ExecutionContext): boolean {
    const user = ctx.switchToHttp().getRequest<{ user?: SessionUser }>().user;
    if (!user || user.token || user.role !== 'admin') {
      throw new ForbiddenException('Solo el administrador puede hacer esto');
    }
    return true;
  }
}

export const CurrentUser = createParamDecorator((_: unknown, ctx: ExecutionContext): SessionUser => {
  return ctx.switchToHttp().getRequest<{ user: SessionUser }>().user;
});
