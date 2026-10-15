import {
  CanActivate,
  createParamDecorator,
  ExecutionContext,
  Injectable,
  UnauthorizedException,
} from '@nestjs/common';
import type { Request } from 'express';
import jwt from 'jsonwebtoken';
import { config } from '../config';

export const SESSION_COOKIE = 'rv_session';

export type SessionUser = { id: string; email: string; role: 'admin' | 'user' };

export function signSession(user: SessionUser): string {
  return jwt.sign(user, config.jwtSecret, { expiresIn: `${config.sessionDays}d` });
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

@Injectable()
export class AuthGuard implements CanActivate {
  canActivate(ctx: ExecutionContext): boolean {
    const req = ctx.switchToHttp().getRequest<Request & { user?: SessionUser }>();
    const user = readSession(req);
    if (!user) throw new UnauthorizedException('Sesión no válida o expirada');
    req.user = user;
    return true;
  }
}

export const CurrentUser = createParamDecorator((_: unknown, ctx: ExecutionContext): SessionUser => {
  return ctx.switchToHttp().getRequest<{ user: SessionUser }>().user;
});
