import type { MailAccount } from '@rvmail/db';
import { decryptSecret } from '@rvmail/shared';
import { ImapFlow } from 'imapflow';
import nodemailer from 'nodemailer';

export type ConnectionSettings = Pick<
  MailAccount,
  'imapHost' | 'imapPort' | 'imapSecure' | 'smtpHost' | 'smtpPort' | 'smtpSecure' | 'username'
> & { password: string };

export function settingsFromAccount(a: MailAccount): ConnectionSettings {
  return { ...a, password: decryptSecret(a.secretEnc) };
}

export function imapClient(s: ConnectionSettings): ImapFlow {
  return new ImapFlow({
    host: s.imapHost,
    port: s.imapPort,
    secure: s.imapSecure,
    auth: { user: s.username, pass: s.password },
    logger: false,
    connectionTimeout: 15_000,
    greetingTimeout: 15_000,
    socketTimeout: 60_000,
    // Permite servidores con certificado autofirmado solo si se configura explícitamente
    tls: { rejectUnauthorized: process.env.IMAP_ALLOW_SELF_SIGNED !== 'true' },
  });
}

export function smtpTransport(s: ConnectionSettings) {
  return nodemailer.createTransport({
    host: s.smtpHost,
    port: s.smtpPort,
    secure: s.smtpSecure,
    auth: { user: s.username, pass: s.password },
    connectionTimeout: 15_000,
    tls: { rejectUnauthorized: process.env.IMAP_ALLOW_SELF_SIGNED !== 'true' },
  });
}

/** Ejecuta una operación con una conexión IMAP corta y siempre la cierra. */
export async function withImap<T>(s: ConnectionSettings, fn: (c: ImapFlow) => Promise<T>): Promise<T> {
  const client = imapClient(s);
  await client.connect();
  try {
    return await fn(client);
  } finally {
    await client.logout().catch(() => client.close());
  }
}

/** Traduce errores técnicos de IMAP/SMTP a mensajes entendibles. */
export function humanizeMailError(err: unknown): string {
  const e = err as { authenticationFailed?: boolean; code?: string; responseCode?: number; message?: string };
  if (e?.authenticationFailed || e?.responseCode === 535 || e?.code === 'EAUTH') {
    return 'Usuario o contraseña rechazados por el servidor';
  }
  if (e?.code === 'ENOTFOUND') return 'No se encontró el servidor (revisa el nombre del host)';
  if (e?.code === 'ECONNREFUSED') return 'El servidor rechazó la conexión (revisa el puerto)';
  if (e?.code === 'ETIMEDOUT' || e?.code === 'ETIMEOUT') return 'Tiempo de espera agotado al conectar';
  if (e?.code === 'ESOCKET' || /certificate|SSL|TLS/i.test(e?.message ?? '')) {
    return 'Error de TLS/SSL: revisa si el puerto usa conexión segura';
  }
  return e?.message ?? 'Error desconocido';
}
