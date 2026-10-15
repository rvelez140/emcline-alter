export type User = { id: string; name: string; email: string; role: 'admin' | 'user' };

export type Account = {
  id: string;
  displayName: string;
  email: string;
  color: string;
  imapHost: string;
  imapPort: number;
  imapSecure: boolean;
  smtpHost: string;
  smtpPort: number;
  smtpSecure: boolean;
  username: string;
  enabled: boolean;
  status: string;
  lastError: string | null;
  lastSyncAt: string | null;
  signatureHtml: string | null;
  source: 'external' | 'mailserver';
  authType: 'password' | 'oauth2';
};

export type SsoProvider = { id: 'google' | 'microsoft' | 'yahoo'; name: string };
export type LinkedIdentity = {
  id: string;
  provider: SsoProvider['id'];
  email: string | null;
  createdAt: string;
  lastLoginAt: string | null;
};

export type Folder = {
  id: string;
  accountId: string;
  path: string;
  name: string;
  specialUse: string | null;
  unreadCount: number;
  totalCount: number;
};

export type Address = { name?: string | null; address: string };

export type MessageSummary = {
  id: string;
  accountId: string;
  folderId: string;
  subject: string;
  fromName: string | null;
  fromAddress: string | null;
  to: Address[];
  date: string;
  snippet: string;
  seen: boolean;
  flagged: boolean;
  hasAttachments: boolean;
  accountColor: string;
  accountEmail: string;
};

export type MessageFull = MessageSummary & {
  cc: Address[];
  html: string | null;
  text: string | null;
  messageId: string | null;
  attachments: Array<{ filename: string | null; contentType: string; size: number }>;
};

export type ApiScope = 'mail:read' | 'mail:draft' | 'mail:send' | 'mail:modify';

export type ApiToken = {
  id: string;
  name: string;
  prefix: string;
  scopes: ApiScope[];
  accountIds: string[] | null;
  lastUsedAt: string | null;
  expiresAt: string | null;
  createdAt: string;
};

export type MailServerStatus = {
  provider: 'none' | 'aapanel';
  enabled: boolean;
  hostname: string;
  ok: boolean;
  error: string | null;
  domains?: number;
};
export type MailDomain = { domain: string; mailboxes: number | null; quotaGb: number | null; createdAt: string | null };
export type Mailbox = {
  email: string;
  fullName: string;
  quotaGb: number | null;
  active: boolean;
  createdAt: string | null;
};
export type DnsCheck = {
  domain: string;
  serverHost: string;
  serverIps: string[];
  checks: Array<{ record: string; ok: boolean; found: string[]; expected: string }>;
};
export type AutoConfig =
  | { managed: false }
  | {
      managed: true;
      imapHost: string;
      imapPort: number;
      imapSecure: boolean;
      smtpHost: string;
      smtpPort: number;
      smtpSecure: boolean;
    };

export type Special = 'inbox' | 'sent' | 'drafts' | 'trash' | 'junk' | 'archive';

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

async function request<T>(method: string, url: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${url}`, {
    method,
    credentials: 'include',
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (res.status === 204) return undefined as T;
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    const msg = Array.isArray(data.message) ? data.message.join(', ') : data.message;
    throw new ApiError(res.status, msg || `Error ${res.status}`);
  }
  return data as T;
}

export const api = {
  setup: () =>
    request<{ needsSetup: boolean; allowSignup: boolean; mailboxLogin: boolean; sso: SsoProvider[] }>(
      'GET',
      '/auth/setup',
    ),
  identities: () => request<{ identities: LinkedIdentity[]; providers: SsoProvider[] }>('GET', '/auth/identities'),
  unlinkIdentity: (id: string) => request<void>('DELETE', `/auth/identities/${id}`),
  oauthProviders: () => request<SsoProvider[]>('GET', '/oauth/providers'),
  me: () => request<User>('GET', '/auth/me'),
  login: (email: string, password: string) => request<User>('POST', '/auth/login', { email, password }),
  register: (name: string, email: string, password: string) =>
    request<User>('POST', '/auth/register', { name, email, password }),
  logout: () => request<void>('POST', '/auth/logout'),

  accounts: () => request<Account[]>('GET', '/accounts'),
  createAccount: (input: Record<string, unknown>) => request<Account>('POST', '/accounts', input),
  updateAccount: (id: string, input: Record<string, unknown>) => request<Account>('PATCH', `/accounts/${id}`, input),
  deleteAccount: (id: string) => request<void>('DELETE', `/accounts/${id}`),

  folders: () => request<Folder[]>('GET', '/folders'),
  messages: (params: { accountId?: string; folderId?: string; special?: Special; q?: string; cursor?: string }) => {
    const qs = new URLSearchParams(
      Object.entries(params).filter((e): e is [string, string] => typeof e[1] === 'string' && e[1] !== ''),
    );
    return request<{ items: MessageSummary[]; nextCursor: string | null }>('GET', `/messages?${qs}`);
  },
  message: (id: string) => request<MessageFull>('GET', `/messages/${id}`),
  patchMessage: (id: string, patch: { seen?: boolean; flagged?: boolean }) =>
    request<MessageSummary>('PATCH', `/messages/${id}`, patch),
  deleteMessage: (id: string) => request<void>('DELETE', `/messages/${id}`),
  send: (input: {
    accountId: string;
    to: string[];
    cc?: string[];
    bcc?: string[];
    subject: string;
    html: string;
    inReplyToMessageId?: string;
  }) => request<{ messageId: string }>('POST', '/send', input),

  tokens: () => request<ApiToken[]>('GET', '/tokens'),
  createToken: (input: {
    name: string;
    scopes: ApiScope[];
    accountIds: string[] | null;
    expiresInDays: number | null;
  }) =>
    request<{ id: string; name: string; prefix: string; scopes: ApiScope[]; token: string }>('POST', '/tokens', input),
  revokeToken: (id: string) => request<void>('DELETE', `/tokens/${id}`),

  autoconfig: (email: string) =>
    request<AutoConfig>('GET', `/mailserver/autoconfig?email=${encodeURIComponent(email)}`),
  mailserver: {
    status: () => request<MailServerStatus>('GET', '/admin/mailserver'),
    domains: () => request<MailDomain[]>('GET', '/admin/mailserver/domains'),
    addDomain: (domain: string) =>
      request<{ domain: string; dns: DnsCheck }>('POST', '/admin/mailserver/domains', { domain }),
    deleteDomain: (domain: string) =>
      request<void>('DELETE', `/admin/mailserver/domains/${encodeURIComponent(domain)}`),
    dns: (domain: string) => request<DnsCheck>('GET', `/admin/mailserver/domains/${encodeURIComponent(domain)}/dns`),
    mailboxes: (domain: string) =>
      request<Mailbox[]>('GET', `/admin/mailserver/domains/${encodeURIComponent(domain)}/mailboxes`),
    users: () => request<Array<{ id: string; name: string; email: string }>>('GET', '/admin/mailserver/users'),
    createMailbox: (input: {
      email: string;
      fullName: string;
      password?: string;
      quotaGb: number;
      link: 'none' | 'me' | 'user' | 'new_user';
      userId?: string;
    }) =>
      request<{ email: string; linkedUserId: string | null; password?: string }>(
        'POST',
        '/admin/mailserver/mailboxes',
        input,
      ),
    updateMailbox: (
      email: string,
      patch: { password?: string; fullName?: string; quotaGb?: number; active?: boolean },
    ) => request<{ email: string }>('PATCH', `/admin/mailserver/mailboxes/${encodeURIComponent(email)}`, patch),
    deleteMailbox: (email: string) =>
      request<void>('DELETE', `/admin/mailserver/mailboxes/${encodeURIComponent(email)}`),
  },
};

export function splitAddresses(s: string): string[] {
  return s
    .split(/[,;\s]+/)
    .map((x) => x.trim())
    .filter(Boolean);
}

export function formatDate(iso: string): string {
  const d = new Date(iso);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString('es-DO', { hour: 'numeric', minute: '2-digit' });
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString('es-DO', { day: 'numeric', month: 'short' });
  }
  return d.toLocaleDateString('es-DO', { day: 'numeric', month: 'short', year: 'numeric' });
}

/** Inicia el flujo OAuth/OIDC (navegación completa: el proveedor vuelve a la web). */
export function ssoStart(provider: string, mode: 'login' | 'link' | 'account') {
  location.href = `/api/auth/sso/${provider}/start?mode=${mode}`;
}

/** Lee y limpia de la URL los avisos que deja el retorno del SSO. */
export function takeSsoNotice(): { error?: string; ok?: string } | null {
  const q = new URLSearchParams(location.search);
  const error = q.get('sso_error') ?? undefined;
  const ok =
    q.get('sso') === 'linked'
      ? 'Cuenta vinculada'
      : q.get('account') === 'added'
        ? 'Cuenta de correo conectada'
        : undefined;
  if (!error && !ok) return null;
  history.replaceState(null, '', location.pathname);
  return { error, ok };
}
