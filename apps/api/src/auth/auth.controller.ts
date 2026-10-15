import {
  Body,
  Controller,
  ForbiddenException,
  Get,
  HttpCode,
  HttpException,
  HttpStatus,
  Inject,
  Post,
  Req,
  Res,
  UnauthorizedException,
  UseGuards,
} from '@nestjs/common';
import { Db, users } from '@rvmail/db';
import argon2 from 'argon2';
import { count, eq } from 'drizzle-orm';
import type { Request, Response } from 'express';
import Redis from 'ioredis';
import { z } from 'zod';
import { parse } from '../common/zod';
import { config } from '../config';
import { DB, REDIS } from '../infra/infra.module';
import { AuthGuard, CurrentUser, SESSION_COOKIE, SessionUser, signSession } from './auth.guard';

const registerSchema = z.object({
  name: z.string().trim().min(1).max(100),
  email: z.string().trim().toLowerCase().email(),
  password: z.string().min(10, 'mínimo 10 caracteres').max(200),
});

const loginSchema = z.object({
  email: z.string().trim().toLowerCase().email(),
  password: z.string().min(1).max(200),
});

@Controller('auth')
export class AuthController {
  constructor(
    @Inject(DB) private readonly db: Db,
    @Inject(REDIS) private readonly redis: Redis,
  ) {}

  /** Indica si la instalación aún no tiene usuarios (pantalla de configuración inicial). */
  @Get('setup')
  async setup() {
    const [{ n }] = await this.db.select({ n: count() }).from(users);
    return { needsSetup: n === 0, allowSignup: config.allowSignup };
  }

  @Post('register')
  async register(@Body() body: unknown, @Res({ passthrough: true }) res: Response) {
    const input = parse(registerSchema, body);
    const [{ n }] = await this.db.select({ n: count() }).from(users);
    if (n > 0 && !config.allowSignup) {
      throw new ForbiddenException('El registro está cerrado. Pide acceso al administrador.');
    }
    const existing = await this.db.query.users.findFirst({ where: eq(users.email, input.email) });
    if (existing) throw new ForbiddenException('Ya existe un usuario con ese correo');

    const [user] = await this.db
      .insert(users)
      .values({
        name: input.name,
        email: input.email,
        passwordHash: await argon2.hash(input.password, { type: argon2.argon2id }),
        role: n === 0 ? 'admin' : 'user',
      })
      .returning();
    this.setCookie(res, { id: user.id, email: user.email, role: user.role });
    return { id: user.id, name: user.name, email: user.email, role: user.role };
  }

  @Post('login')
  @HttpCode(200)
  async login(@Body() body: unknown, @Req() req: Request, @Res({ passthrough: true }) res: Response) {
    const input = parse(loginSchema, body);
    const rlKey = `rvmail:rl:login:${req.ip}:${input.email}`;
    const attempts = await this.redis.incr(rlKey);
    if (attempts === 1) await this.redis.expire(rlKey, 15 * 60);
    if (attempts > 10) {
      throw new HttpException('Demasiados intentos. Espera 15 minutos.', HttpStatus.TOO_MANY_REQUESTS);
    }

    const user = await this.db.query.users.findFirst({ where: eq(users.email, input.email) });
    if (!user || !(await argon2.verify(user.passwordHash, input.password))) {
      throw new UnauthorizedException('Correo o contraseña incorrectos');
    }
    await this.redis.del(rlKey);
    this.setCookie(res, { id: user.id, email: user.email, role: user.role });
    return { id: user.id, name: user.name, email: user.email, role: user.role };
  }

  @Post('logout')
  @HttpCode(204)
  logout(@Res({ passthrough: true }) res: Response) {
    res.clearCookie(SESSION_COOKIE, { path: '/' });
  }

  @Get('me')
  @UseGuards(AuthGuard)
  async me(@CurrentUser() session: SessionUser) {
    const user = await this.db.query.users.findFirst({ where: eq(users.id, session.id) });
    if (!user) throw new UnauthorizedException();
    return { id: user.id, name: user.name, email: user.email, role: user.role };
  }

  private setCookie(res: Response, user: SessionUser) {
    res.cookie(SESSION_COOKIE, signSession(user), {
      httpOnly: true,
      sameSite: 'lax',
      secure: config.secureCookies,
      path: '/',
      maxAge: config.sessionDays * 24 * 3600 * 1000,
    });
  }
}
