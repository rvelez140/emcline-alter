import { BadGatewayException, Inject, Injectable, Logger, ServiceUnavailableException } from '@nestjs/common';
import Redis from 'ioredis';
import { resolve4, resolveMx, resolveTxt } from 'node:dns/promises';
import { config } from '../config';
import { REDIS } from '../infra/infra.module';
import { AaPanelClient, AaPanelError, listFrom } from './aapanel.client';

export type MailDomain = { domain: string; mailboxes: number | null; quotaGb: number | null; createdAt: string | null };
export type Mailbox = { email: string; fullName: string; quotaGb: number | null; active: boolean; createdAt: string | null };

/** Contrato de un servidor de correo administrable (hoy aaPanel; mañana Stalwart, Mailcow…). */
export interface MailServerProvider {
  listDomains(): Promise<MailDomain[]>;
  addDomain(domain: string): Promise<void>;
  deleteDomain(domain: string): Promise<void>;
  listMailboxes(domain: string): Promise<Mailbox[]>;
  addMailbox(m: { email: string; fullName: string; password: string; quotaGb: number }): Promise<void>;
  updateMailbox(
    email: string,
    current: Mailbox,
    patch: { password?: string; fullName?: string; quotaGb?: number; active?: boolean },
  ): Promise<void>;
  deleteMailbox(email: string): Promise<void>;
}

/**
 * Funciones del plugin "mail_sys" (Mail Server) de aaPanel. Si tu versión del
 * plugin usa otros nombres, compruébalos en el VPS con:
 *   grep -n "def " /www/server/panel/plugin/mail_sys/mail_sys_main.py
 */
const MAIL_SYS = 'mail_sys';
const FN = {
  listDomains: 'get_domains',
  addDomain: 'add_domain_new',
  deleteDomain: 'delete_domain',
  listMailboxes: 'get_mailboxs',
  addMailbox: 'add_mailbox',
  updateMailbox: 'update_mailbox',
  deleteMailbox: 'delete_mailbox',
} as const;

const num = (v: unknown): number | null => {
  const n = typeof v === 'number' ? v : parseFloat(String(v ?? ''));
  return Number.isFinite(n) ? n : null;
};
/** aaPanel expresa las cuotas como "5 GB" / bytes según la versión. */
const quotaGb = (v: unknown): number | null => {
  if (typeof v === 'string' && /gb/i.test(v)) return num(v);
  const n = num(v);
  if (n === null) return null;
  return n > 1024 * 1024 ? Math.round((n / 1024 ** 3) * 10) / 10 : n;
};

class AaPanelProvider implements MailServerProvider {
  constructor(private readonly api: AaPanelClient) {}

  async listDomains(): Promise<MailDomain[]> {
    const data = await this.api.plugin(MAIL_SYS, FN.listDomains, { p: 1, size: 1000 });
    return listFrom<Record<string, unknown>>(data).map((d) => ({
      domain: String(d.domain ?? ''),
      mailboxes: num(d.mailbox_count ?? d.mailboxes),
      quotaGb: quotaGb(d.quota),
      createdAt: d.created ? String(d.created) : null,
    }));
  }

  async addDomain(domain: string) {
    // a_record = host del servidor de correo al que apuntará el MX
    await this.api.plugin(MAIL_SYS, FN.addDomain, {
      domain,
      a_record: config.mailserver.hostname || `mail.${domain}`,
      mailboxes: 50,
      mailbox_quota: '5 GB',
      quota: '50 GB',
    });
  }

  async deleteDomain(domain: string) {
    await this.api.plugin(MAIL_SYS, FN.deleteDomain, { domain });
  }

  async listMailboxes(domain: string): Promise<Mailbox[]> {
    const data = await this.api.plugin(MAIL_SYS, FN.listMailboxes, { domain, p: 1, size: 1000 });
    return listFrom<Record<string, unknown>>(data).map((m) => ({
      email: String(m.username ?? m.email ?? ''),
      fullName: String(m.full_name ?? m.name ?? ''),
      quotaGb: quotaGb(m.quota),
      active: String(m.active ?? '1') !== '0',
      createdAt: m.created ? String(m.created) : null,
    }));
  }

  async addMailbox(m: { email: string; fullName: string; password: string; quotaGb: number }) {
    await this.api.plugin(MAIL_SYS, FN.addMailbox, {
      username: m.email,
      full_name: m.fullName,
      password: m.password,
      quota: `${m.quotaGb} GB`,
      is_admin: 0,
      active: 1,
    });
  }

  async updateMailbox(
    email: string,
    current: Mailbox,
    patch: { password?: string; fullName?: string; quotaGb?: number; active?: boolean },
  ) {
    // update_mailbox espera el registro completo: se combinan valores actuales y cambios
    await this.api.plugin(MAIL_SYS, FN.updateMailbox, {
      username: email,
      full_name: patch.fullName ?? current.fullName,
      quota: `${patch.quotaGb ?? current.quotaGb ?? 5} GB`,
      active: (patch.active ?? current.active) ? 1 : 0,
      is_admin: 0,
      ...(patch.password ? { password: patch.password } : {}),
    });
  }

  async deleteMailbox(email: string) {
    await this.api.plugin(MAIL_SYS, FN.deleteMailbox, { username: email });
  }
}

export type DnsCheck = {
  domain: string;
  serverHost: string;
  serverIps: string[];
  checks: Array<{ record: string; ok: boolean; found: string[]; expected: string }>;
};

/**
 * Servidor de correo integrado. Si MAILSERVER_PROVIDER=none, RV Mail funciona
 * solo como cliente y todos estos métodos responden "no configurado".
 */
@Injectable()
export class MailServerService {
  private readonly log = new Logger('MailServer');
  private readonly provider: MailServerProvider | null;

  constructor(@Inject(REDIS) private readonly redis: Redis) {
    const c = config.mailserver;
    this.provider =
      c.provider === 'aapanel' && c.aapanelApiKey
        ? new AaPanelProvider(new AaPanelClient(c.aapanelUrl, c.aapanelApiKey, c.aapanelVerifyTls))
        : null;
    if (c.provider === 'aapanel' && !c.aapanelApiKey) {
      this.log.warn('MAILSERVER_PROVIDER=aapanel pero falta AAPANEL_API_KEY: integración desactivada');
    }
  }

  get enabled(): boolean {
    return this.provider !== null;
  }

  /** Datos de conexión IMAP/SMTP de los buzones del servidor integrado. */
  get connection() {
    const c = config.mailserver;
    return {
      imapHost: c.hostname,
      imapPort: c.imapPort,
      imapSecure: c.imapPort === 993,
      smtpHost: c.hostname,
      smtpPort: c.smtpPort,
      smtpSecure: c.smtpPort === 465,
    };
  }

  private get p(): MailServerProvider {
    if (!this.provider) throw new ServiceUnavailableException('El servidor de correo integrado no está configurado');
    return this.provider;
  }

  /** Ejecuta una llamada al panel traduciendo sus errores a mensajes para el usuario. */
  private async call<T>(fn: (p: MailServerProvider) => Promise<T>): Promise<T> {
    try {
      return await fn(this.p);
    } catch (e) {
      if (e instanceof AaPanelError) throw new BadGatewayException(`Servidor de correo: ${e.message}`);
      throw e;
    }
  }

  async status() {
    const c = config.mailserver;
    if (!this.provider) return { provider: c.provider, enabled: false, hostname: c.hostname, ok: false, error: null };
    try {
      const domains = await this.provider.listDomains();
      return { provider: c.provider, enabled: true, hostname: c.hostname, ok: true, error: null, domains: domains.length };
    } catch (e) {
      const error = e instanceof AaPanelError ? e.message : 'Error desconocido';
      return { provider: c.provider, enabled: true, hostname: c.hostname, ok: false, error };
    }
  }

  listDomains() {
    return this.call((p) => p.listDomains());
  }

  async addDomain(domain: string) {
    await this.call((p) => p.addDomain(domain));
    await this.redis.del(DOMAINS_CACHE);
  }

  async deleteDomain(domain: string) {
    await this.call((p) => p.deleteDomain(domain));
    await this.redis.del(DOMAINS_CACHE);
  }

  listMailboxes(domain: string) {
    return this.call((p) => p.listMailboxes(domain));
  }

  addMailbox(m: { email: string; fullName: string; password: string; quotaGb: number }) {
    return this.call((p) => p.addMailbox(m));
  }

  async updateMailbox(email: string, patch: { password?: string; fullName?: string; quotaGb?: number; active?: boolean }) {
    const domain = email.split('@')[1];
    const current = (await this.listMailboxes(domain)).find((m) => m.email.toLowerCase() === email.toLowerCase());
    if (!current) throw new BadGatewayException('Servidor de correo: el buzón no existe');
    await this.call((p) => p.updateMailbox(email, current, patch));
  }

  deleteMailbox(email: string) {
    return this.call((p) => p.deleteMailbox(email));
  }

  /** ¿El dominio está alojado en el servidor integrado? (caché de 5 min en Redis) */
  async isManagedDomain(domain: string): Promise<boolean> {
    if (!this.provider || !config.mailserver.hostname) return false;
    let list: string[];
    const cached = await this.redis.get(DOMAINS_CACHE);
    if (cached) list = JSON.parse(cached);
    else {
      try {
        list = (await this.provider.listDomains()).map((d) => d.domain.toLowerCase());
      } catch (e) {
        this.log.warn(`no se pudo leer la lista de dominios: ${(e as Error).message}`);
        return false;
      }
      await this.redis.set(DOMAINS_CACHE, JSON.stringify(list), 'EX', 300);
    }
    return list.includes(domain.toLowerCase());
  }

  /**
   * Verifica los registros DNS que necesita un dominio para recibir y enviar
   * correo sin caer en spam: MX hacia este servidor, SPF, DKIM y DMARC.
   */
  async checkDns(domain: string): Promise<DnsCheck> {
    const serverHost = config.mailserver.hostname || `mail.${domain}`;
    const safe = <T>(p: Promise<T>, fallback: T) => p.catch(() => fallback);
    const flat = (txt: string[][]) => txt.map((t) => t.join(''));

    const [serverIps, mx, txt, dmarc, dkim] = await Promise.all([
      safe(resolve4(serverHost), [] as string[]),
      safe(resolveMx(domain), [] as { exchange: string; priority: number }[]),
      safe(resolveTxt(domain).then(flat), [] as string[]),
      safe(resolveTxt(`_dmarc.${domain}`).then(flat), [] as string[]),
      safe(resolveTxt(`default._domainkey.${domain}`).then(flat), [] as string[]),
    ]);

    const mxIps = (
      await Promise.all(mx.map((m) => safe(resolve4(m.exchange), [] as string[])))
    ).flat();
    const spf = txt.filter((t) => t.toLowerCase().startsWith('v=spf1'));

    return {
      domain,
      serverHost,
      serverIps,
      checks: [
        {
          record: 'A (servidor de correo)',
          ok: serverIps.length > 0,
          found: serverIps,
          expected: `${serverHost} → IP pública del VPS`,
        },
        {
          record: 'MX',
          ok: mx.length > 0 && mxIps.some((ip) => serverIps.includes(ip)),
          found: mx.sort((a, b) => a.priority - b.priority).map((m) => `${m.priority} ${m.exchange}`),
          expected: `10 ${serverHost}`,
        },
        {
          record: 'SPF (TXT)',
          ok: spf.length === 1,
          found: spf,
          expected: 'v=spf1 mx a ~all   (un único registro SPF)',
        },
        {
          record: 'DKIM (default._domainkey)',
          ok: dkim.some((t) => /v=DKIM1/i.test(t) || /p=/.test(t)),
          found: dkim.map((t) => (t.length > 60 ? `${t.slice(0, 60)}…` : t)),
          expected: 'Copia la clave DKIM desde aaPanel → Mail Server → dominio → DNS',
        },
        {
          record: 'DMARC (_dmarc)',
          ok: dmarc.some((t) => t.toLowerCase().startsWith('v=dmarc1')),
          found: dmarc,
          expected: `v=DMARC1; p=quarantine; rua=mailto:postmaster@${domain}`,
        },
      ],
    };
  }
}

const DOMAINS_CACHE = 'rvmail:mailserver:domains';
