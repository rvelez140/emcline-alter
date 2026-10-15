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
  setup: () => request<{ needsSetup: boolean; allowSignup: boolean }>('GET', '/auth/setup'),
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
