import { Inject, Injectable } from '@nestjs/common';
import { Db, mailAccounts } from '@rvmail/db';
import { encryptSecret, workerControlChannel, WorkerControl } from '@rvmail/shared';
import { and, eq } from 'drizzle-orm';
import Redis from 'ioredis';
import { DB, REDIS } from '../infra/infra.module';
import { withImap } from '../mail/mail-clients';
import { MailServerService } from './mailserver.service';

/**
 * Conecta buzones del servidor integrado como cuentas de RV Mail, para que
 * todo buzón alojado se use con todas las funciones del cliente.
 */
@Injectable()
export class ProvisioningService {
  constructor(
    @Inject(DB) private readonly db: Db,
    @Inject(REDIS) private readonly redis: Redis,
    private readonly mailserver: MailServerService,
  ) {}

  /** ¿Usuario y contraseña válidos en el IMAP del servidor integrado? */
  async verifyMailboxLogin(email: string, password: string): Promise<boolean> {
    try {
      await withImap({ ...this.mailserver.connection, username: email, password }, async () => undefined);
      return true;
    } catch {
      return false;
    }
  }

  /** Crea (o actualiza la contraseña de) la cuenta del buzón para el usuario. */
  async linkMailbox(userId: string, email: string, password: string, displayName: string) {
    const existing = await this.db.query.mailAccounts.findFirst({
      where: and(eq(mailAccounts.userId, userId), eq(mailAccounts.email, email)),
    });
    let accountId: string;
    if (existing) {
      await this.db
        .update(mailAccounts)
        .set({ secretEnc: encryptSecret(password), status: 'pending', lastError: null, enabled: true })
        .where(eq(mailAccounts.id, existing.id));
      accountId = existing.id;
    } else {
      const [row] = await this.db
        .insert(mailAccounts)
        .values({
          userId,
          displayName: displayName || email,
          email,
          username: email,
          secretEnc: encryptSecret(password),
          source: 'mailserver',
          ...this.mailserver.connection,
        })
        .returning();
      accountId = row.id;
    }
    await this.notify({ type: 'account.changed', accountId });
    return accountId;
  }

  /** Tras cambiar la contraseña en el servidor, actualiza todas las cuentas que usan ese buzón. */
  async updateLinkedPassword(email: string, password: string) {
    const rows = await this.db
      .update(mailAccounts)
      .set({ secretEnc: encryptSecret(password), status: 'pending', lastError: null })
      .where(and(eq(mailAccounts.email, email), eq(mailAccounts.source, 'mailserver')))
      .returning({ id: mailAccounts.id });
    for (const r of rows) await this.notify({ type: 'account.changed', accountId: r.id });
  }

  /** Al desactivar/borrar un buzón, sus cuentas vinculadas dejan de sincronizar. */
  async setLinkedEnabled(email: string, enabled: boolean) {
    const rows = await this.db
      .update(mailAccounts)
      .set({ enabled })
      .where(and(eq(mailAccounts.email, email), eq(mailAccounts.source, 'mailserver')))
      .returning({ id: mailAccounts.id });
    for (const r of rows) await this.notify({ type: 'account.changed', accountId: r.id });
  }

  private async notify(msg: WorkerControl) {
    await this.redis.publish(workerControlChannel, JSON.stringify(msg));
  }
}
