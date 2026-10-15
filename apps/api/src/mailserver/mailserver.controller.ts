import {
  BadRequestException,
  Body,
  ConflictException,
  Controller,
  Delete,
  Get,
  HttpCode,
  Inject,
  Param,
  Patch,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { Db, users } from '@rvmail/db';
import argon2 from 'argon2';
import { asc, eq } from 'drizzle-orm';
import { randomBytes } from 'node:crypto';
import { z } from 'zod';
import { AdminGuard, AuthGuard, CurrentUser, SessionUser } from '../auth/auth.guard';
import { parse } from '../common/zod';
import { DB } from '../infra/infra.module';
import { MailServerService } from './mailserver.service';
import { ProvisioningService } from './provisioning.service';

const domainRe = /^(?=.{3,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const domainSchema = z.string().trim().toLowerCase().regex(domainRe, 'dominio no válido');
const passwordSchema = z.string().min(10, 'mínimo 10 caracteres').max(200);

const createMailboxSchema = z.object({
  email: z.string().trim().toLowerCase().email(),
  fullName: z.string().trim().min(1).max(100),
  /** Si se omite, se genera una contraseña segura y se devuelve una sola vez */
  password: passwordSchema.optional(),
  quotaGb: z.number().min(0.1).max(1024).default(5),
  /**
   * Qué hacer en RV Mail con el buzón:
   *  none     → solo crearlo en el servidor
   *  me       → conectarlo a la cuenta del administrador que lo crea
   *  user     → conectarlo a un usuario existente (userId)
   *  new_user → crear un usuario de RV Mail que inicia sesión con el mismo correo y contraseña
   */
  link: z.enum(['none', 'me', 'user', 'new_user']).default('new_user'),
  userId: z.string().uuid().optional(),
});

const updateMailboxSchema = z.object({
  password: passwordSchema.optional(),
  fullName: z.string().trim().min(1).max(100).optional(),
  quotaGb: z.number().min(0.1).max(1024).optional(),
  active: z.boolean().optional(),
});

function generatePassword(): string {
  // 18 caracteres sin símbolos ambiguos; los servidores de correo los aceptan todos
  return randomBytes(14).toString('base64').replace(/[+/=]/g, '').slice(0, 16) + 'A9';
}

/** Administración del servidor de correo integrado (solo administradores). */
@Controller('admin/mailserver')
@UseGuards(AuthGuard, AdminGuard)
export class MailServerAdminController {
  constructor(
    @Inject(DB) private readonly db: Db,
    private readonly mailserver: MailServerService,
    private readonly provisioning: ProvisioningService,
  ) {}

  @Get()
  status() {
    return this.mailserver.status();
  }

  @Get('domains')
  domains() {
    return this.mailserver.listDomains();
  }

  @Post('domains')
  async addDomain(@Body() body: unknown) {
    const { domain } = parse(z.object({ domain: domainSchema }), body);
    await this.mailserver.addDomain(domain);
    return { domain, dns: await this.mailserver.checkDns(domain) };
  }

  @Delete('domains/:domain')
  @HttpCode(204)
  async deleteDomain(@Param('domain') raw: string) {
    await this.mailserver.deleteDomain(parse(domainSchema, raw));
  }

  @Get('domains/:domain/dns')
  dns(@Param('domain') raw: string) {
    return this.mailserver.checkDns(parse(domainSchema, raw));
  }

  @Get('domains/:domain/mailboxes')
  mailboxes(@Param('domain') raw: string) {
    return this.mailserver.listMailboxes(parse(domainSchema, raw));
  }

  /** Usuarios de RV Mail, para asignarles buzones. */
  @Get('users')
  users() {
    return this.db
      .select({ id: users.id, name: users.name, email: users.email, role: users.role })
      .from(users)
      .orderBy(asc(users.name));
  }

  @Post('mailboxes')
  async createMailbox(@CurrentUser() admin: SessionUser, @Body() body: unknown) {
    const input = parse(createMailboxSchema, body);
    const domain = input.email.split('@')[1];
    if (!(await this.mailserver.isManagedDomain(domain))) {
      throw new BadRequestException(`El dominio ${domain} no está alojado en este servidor`);
    }
    if (input.link === 'user' && !input.userId) throw new BadRequestException('Falta el usuario al que asignar el buzón');
    if (input.link === 'new_user') {
      const exists = await this.db.query.users.findFirst({ where: eq(users.email, input.email) });
      if (exists) throw new ConflictException('Ya existe un usuario de RV Mail con ese correo; usa "asignar a usuario"');
    }

    const generated = !input.password;
    const password = input.password ?? generatePassword();
    await this.mailserver.addMailbox({ email: input.email, fullName: input.fullName, password, quotaGb: input.quotaGb });

    let userId: string | null = null;
    if (input.link === 'me') userId = admin.id;
    if (input.link === 'user') userId = input.userId!;
    if (input.link === 'new_user') {
      const [u] = await this.db
        .insert(users)
        .values({
          email: input.email,
          name: input.fullName,
          passwordHash: await argon2.hash(password, { type: argon2.argon2id }),
          role: 'user',
        })
        .returning();
      userId = u.id;
    }
    if (userId) await this.provisioning.linkMailbox(userId, input.email, password, input.fullName);

    return { email: input.email, linkedUserId: userId, ...(generated ? { password } : {}) };
  }

  @Patch('mailboxes/:email')
  async updateMailbox(@Param('email') rawEmail: string, @Body() body: unknown) {
    const email = parse(z.string().trim().toLowerCase().email(), rawEmail);
    const patch = parse(updateMailboxSchema, body);
    await this.mailserver.updateMailbox(email, patch);
    if (patch.password) await this.provisioning.updateLinkedPassword(email, patch.password);
    if (patch.active !== undefined) await this.provisioning.setLinkedEnabled(email, patch.active);
    return { email };
  }

  @Delete('mailboxes/:email')
  @HttpCode(204)
  async deleteMailbox(@Param('email') rawEmail: string) {
    const email = parse(z.string().trim().toLowerCase().email(), rawEmail);
    await this.mailserver.deleteMailbox(email);
    await this.provisioning.setLinkedEnabled(email, false);
  }
}

/** Datos públicos para usuarios: autoconfiguración de cuentas del servidor integrado. */
@Controller('mailserver')
@UseGuards(AuthGuard)
export class MailServerPublicController {
  constructor(private readonly mailserver: MailServerService) {}

  @Get('autoconfig')
  async autoconfig(@Query('email') rawEmail: string) {
    const email = parse(z.string().trim().toLowerCase().email(), rawEmail ?? '');
    const managed = await this.mailserver.isManagedDomain(email.split('@')[1]);
    return managed ? { managed: true, ...this.mailserver.connection } : { managed: false };
  }
}
