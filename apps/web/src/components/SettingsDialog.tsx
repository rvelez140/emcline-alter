import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Bot,
  Check,
  CheckCircle2,
  Copy,
  Globe,
  KeyRound,
  Laptop,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  X,
  XCircle,
} from 'lucide-react';
import { FormEvent, ReactNode, useState } from 'react';
import { Account, api, ApiScope, DnsCheck, formatDate, User } from '../api';
import { isNativeApp, registerMailtoHandler } from '../native';
import { SsoButtons } from './SsoButtons';

type Tab = 'ai' | 'login' | 'mailserver';

const SCOPES: Array<{ scope: ApiScope; label: string; hint: string }> = [
  { scope: 'mail:read', label: 'Leer correo', hint: 'Buscar y leer mensajes y carpetas' },
  {
    scope: 'mail:draft',
    label: 'Crear borradores',
    hint: 'Deja respuestas en Borradores para que tú las revises y envíes',
  },
  { scope: 'mail:send', label: 'Enviar correo', hint: 'Envía directamente, sin revisión humana' },
  { scope: 'mail:modify', label: 'Organizar', hint: 'Marcar leído/destacado y mover a la papelera' },
];

export function SettingsDialog({ user, accounts, onClose }: { user: User; accounts: Account[]; onClose: () => void }) {
  const [tab, setTab] = useState<Tab>('ai');
  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/30 md:items-center md:p-6">
      <div className="flex h-[92vh] w-full flex-col overflow-hidden bg-white shadow-2xl md:h-[85vh] md:max-w-3xl md:rounded-2xl dark:bg-slate-900">
        <div className="flex items-center justify-between border-b border-slate-200 px-4 py-3 dark:border-slate-800">
          <h3 className="font-semibold">Configuración</h3>
          <button type="button" className="btn-ghost" onClick={onClose} aria-label="Cerrar">
            <X size={18} />
          </button>
        </div>
        <div className="flex gap-1 border-b border-slate-200 px-3 pt-2 dark:border-slate-800">
          <TabButton active={tab === 'ai'} onClick={() => setTab('ai')} icon={<Bot size={16} />}>
            Acceso para IA
          </TabButton>
          <TabButton active={tab === 'login'} onClick={() => setTab('login')} icon={<KeyRound size={16} />}>
            Inicio de sesión
          </TabButton>
          {user.role === 'admin' && (
            <TabButton active={tab === 'mailserver'} onClick={() => setTab('mailserver')} icon={<Server size={16} />}>
              Servidor de correo
            </TabButton>
          )}
        </div>
        <div className="flex-1 overflow-y-auto p-4">
          {tab === 'ai' && <AiAccess accounts={accounts} />}
          {tab === 'login' && <LoginMethods />}
          {tab === 'mailserver' && <MailServerAdmin />}
        </div>
      </div>
    </div>
  );
}

function TabButton(props: { active: boolean; onClick: () => void; icon: ReactNode; children: ReactNode }) {
  return (
    <button
      onClick={props.onClick}
      className={`flex items-center gap-2 rounded-t-lg border-b-2 px-3 py-2 text-sm ${
        props.active
          ? 'border-brand-600 font-medium text-brand-700 dark:text-brand-100'
          : 'border-transparent text-slate-500 hover:text-slate-800 dark:hover:text-slate-200'
      }`}
    >
      {props.icon}
      {props.children}
    </button>
  );
}

function CopyBox({ value, multiline }: { value: string; multiline?: boolean }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="relative">
      <pre
        className={`rounded-lg bg-slate-100 p-2 pr-10 font-mono text-xs dark:bg-slate-800 ${multiline ? 'whitespace-pre-wrap' : 'overflow-x-auto'} break-all`}
      >
        {value}
      </pre>
      <button
        type="button"
        className="btn-ghost absolute top-1 right-1 p-1"
        aria-label="Copiar"
        onClick={() => {
          void navigator.clipboard?.writeText(value);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }}
      >
        {copied ? <Check size={14} /> : <Copy size={14} />}
      </button>
    </div>
  );
}

/* ------------------------------------------------------------------ inicio de sesión */

const PROVIDER_NAMES: Record<string, string> = { google: 'Google', microsoft: 'Microsoft', yahoo: 'Yahoo' };

function LoginMethods() {
  const qc = useQueryClient();
  const data = useQuery({ queryKey: ['identities'], queryFn: api.identities });
  const unlink = useMutation({
    mutationFn: (id: string) => api.unlinkIdentity(id),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['identities'] }),
  });
  const providers = data.data?.providers ?? [];
  const linked = data.data?.identities ?? [];

  if (data.isSuccess && providers.length === 0 && linked.length === 0) {
    return (
      <div className="space-y-6">
        <p className="text-sm text-slate-500">
          El administrador no ha configurado el inicio de sesión con Google, Microsoft o Yahoo (ver{' '}
          <code>docs/SSO.md</code>
          ).
        </p>
        <Devices />
      </div>
    );
  }
  return (
    <div className="space-y-5 text-sm">
      <p className="text-slate-600 dark:text-slate-400">
        Vincula tu cuenta de Google, Microsoft o Yahoo para entrar a RV Mail sin escribir la contraseña.
      </p>
      <div className="space-y-2">
        <h4 className="font-medium">Cuentas vinculadas</h4>
        {linked.length === 0 && <p className="text-slate-500">Ninguna todavía.</p>}
        {linked.map((i) => (
          <div
            key={i.id}
            className="flex items-center gap-3 rounded-lg border border-slate-200 p-2 dark:border-slate-800"
          >
            <KeyRound size={18} className="shrink-0 text-slate-400" />
            <div className="min-w-0 flex-1">
              <p className="font-medium">{PROVIDER_NAMES[i.provider] ?? i.provider}</p>
              <p className="truncate text-xs text-slate-500">
                {i.email ?? 'sin correo'} ·{' '}
                {i.lastLoginAt ? `último acceso ${formatDate(i.lastLoginAt)}` : 'sin usar aún'}
              </p>
            </div>
            <button
              className="btn-ghost text-red-600"
              onClick={() => confirm('¿Desvincular esta cuenta?') && unlink.mutate(i.id)}
            >
              Desvincular
            </button>
          </div>
        ))}
        {unlink.isError && <p className="text-red-600">{(unlink.error as Error).message}</p>}
      </div>
      {providers.length > 0 && (
        <div className="max-w-xs space-y-2">
          <h4 className="font-medium">Vincular otra</h4>
          <SsoButtons providers={providers} mode="link" label={(n) => `Vincular ${n}`} />
        </div>
      )}
      <Devices />
    </div>
  );
}

/** Apps de escritorio y móvil con sesión abierta (se puede cerrar a distancia). */
function Devices() {
  const qc = useQueryClient();
  const data = useQuery({ queryKey: ['app-sessions'], queryFn: api.appSessions });
  const revoke = useMutation({
    mutationFn: (id: string) => api.revokeAppSession(id),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['app-sessions'] }),
  });
  const list = data.data ?? [];
  return (
    <div className="space-y-2 text-sm">
      <h4 className="font-medium">Apps conectadas</h4>
      {list.length === 0 && <p className="text-slate-500">Ninguna app de escritorio o móvil ha iniciado sesión.</p>}
      {list.map((d) => (
        <div
          key={d.id}
          className="flex items-center gap-3 rounded-lg border border-slate-200 p-2 dark:border-slate-800"
        >
          <Laptop size={18} className="shrink-0 text-slate-400" />
          <div className="min-w-0 flex-1">
            <p className="font-medium">
              {d.deviceName} <span className="font-normal text-slate-500">· {d.platform}</span>
            </p>
            <p className="truncate text-xs text-slate-500">
              Último uso {formatDate(d.lastUsedAt)}
              {d.ip ? ` · ${d.ip}` : ''}
            </p>
          </div>
          <button
            className="btn-ghost text-red-600"
            onClick={() => confirm(`¿Cerrar la sesión de «${d.deviceName}»?`) && revoke.mutate(d.id)}
          >
            Cerrar sesión
          </button>
        </div>
      ))}
      {revoke.isError && <p className="text-red-600">{(revoke.error as Error).message}</p>}
      {!isNativeApp() && 'registerProtocolHandler' in navigator && (
        <button className="btn-ghost mt-2" onClick={registerMailtoHandler}>
          Abrir los enlaces de correo (mailto:) con RV Mail en este navegador
        </button>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ IA */

function AiAccess({ accounts }: { accounts: Account[] }) {
  const qc = useQueryClient();
  const tokens = useQuery({ queryKey: ['tokens'], queryFn: api.tokens });
  const [name, setName] = useState('Claude');
  const [scopes, setScopes] = useState<ApiScope[]>(['mail:read', 'mail:draft']);
  const [accountIds, setAccountIds] = useState<string[]>([]);
  const [expires, setExpires] = useState<number | null>(90);
  const [created, setCreated] = useState<string | null>(null);
  const mcpUrl = `${location.origin}/api/mcp`;

  const create = useMutation({
    mutationFn: () =>
      api.createToken({ name, scopes, accountIds: accountIds.length ? accountIds : null, expiresInDays: expires }),
    onSuccess: (r) => {
      setCreated(r.token);
      void qc.invalidateQueries({ queryKey: ['tokens'] });
    },
  });
  const revoke = useMutation({
    mutationFn: (id: string) => api.revokeToken(id),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['tokens'] }),
  });

  const toggle = <T,>(list: T[], v: T) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);

  function submit(e: FormEvent) {
    e.preventDefault();
    create.mutate();
  }

  const shown = created ?? '<TU_TOKEN>';
  return (
    <div className="space-y-6 text-sm">
      <p className="text-slate-600 dark:text-slate-400">
        Conecta un agente de IA (Claude, u otro compatible con <b>MCP</b>) para que lea y envíe correo a través de RV
        Mail. Cada token tiene permisos y cuentas limitados, y puedes revocarlo cuando quieras. Los envíos de agentes
        tienen un límite por hora.
      </p>

      <form onSubmit={submit} className="space-y-3 rounded-xl border border-slate-200 p-3 dark:border-slate-800">
        <h4 className="font-medium">Nuevo token</h4>
        <div className="grid gap-3 sm:grid-cols-2">
          <div>
            <label className="label">Nombre</label>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} required maxLength={80} />
          </div>
          <div>
            <label className="label">Caduca</label>
            <select
              className="input"
              value={expires ?? ''}
              onChange={(e) => setExpires(e.target.value ? Number(e.target.value) : null)}
            >
              <option value={30}>En 30 días</option>
              <option value={90}>En 90 días</option>
              <option value={365}>En 1 año</option>
              <option value="">Nunca</option>
            </select>
          </div>
        </div>
        <div>
          <label className="label">Permisos</label>
          <div className="grid gap-2 sm:grid-cols-2">
            {SCOPES.map((s) => (
              <label
                key={s.scope}
                className="flex items-start gap-2 rounded-lg border border-slate-200 p-2 dark:border-slate-800"
              >
                <input
                  type="checkbox"
                  className="mt-1"
                  checked={scopes.includes(s.scope)}
                  onChange={() => setScopes(toggle(scopes, s.scope))}
                />
                <span>
                  <span className="font-medium">{s.label}</span>
                  <span className="block text-xs text-slate-500">{s.hint}</span>
                </span>
              </label>
            ))}
          </div>
          {scopes.includes('mail:send') && (
            <p className="mt-1 text-xs text-amber-600">
              Con "Enviar correo" el agente envía sin tu revisión. Para empezar es más seguro "Crear borradores".
            </p>
          )}
        </div>
        <div>
          <label className="label">Cuentas (ninguna marcada = todas)</label>
          <div className="flex flex-wrap gap-2">
            {accounts.map((a) => (
              <label
                key={a.id}
                className="flex items-center gap-1.5 rounded-full border border-slate-200 px-2 py-1 text-xs dark:border-slate-800"
              >
                <input
                  type="checkbox"
                  checked={accountIds.includes(a.id)}
                  onChange={() => setAccountIds(toggle(accountIds, a.id))}
                />
                {a.email}
              </label>
            ))}
          </div>
        </div>
        {create.isError && <p className="text-red-600">{(create.error as Error).message}</p>}
        <button className="btn-primary" disabled={create.isPending || scopes.length === 0}>
          <Plus size={16} /> Crear token
        </button>
      </form>

      {created && (
        <div className="space-y-2 rounded-xl border border-green-300 bg-green-50 p-3 dark:border-green-800 dark:bg-green-900/20">
          <p className="font-medium text-green-800 dark:text-green-200">
            Token creado. Cópialo ahora: no se volverá a mostrar.
          </p>
          <CopyBox value={created} />
        </div>
      )}

      <div className="space-y-2">
        <h4 className="font-medium">Cómo conectar el agente</h4>
        <p className="text-xs text-slate-500">Dirección del servidor MCP (Streamable HTTP):</p>
        <CopyBox value={mcpUrl} />
        <p className="text-xs text-slate-500">Claude Code:</p>
        <CopyBox
          value={`claude mcp add --transport http rvmail ${mcpUrl} --header "Authorization: Bearer ${shown}"`}
          multiline
        />
        <p className="text-xs text-slate-500">Otros clientes MCP (configuración JSON):</p>
        <CopyBox
          multiline
          value={JSON.stringify(
            { mcpServers: { rvmail: { type: 'http', url: mcpUrl, headers: { Authorization: `Bearer ${shown}` } } } },
            null,
            2,
          )}
        />
        <p className="text-xs text-slate-500">
          También puedes usar la API REST con el mismo token: <code>Authorization: Bearer …</code> en{' '}
          <code>/api/messages</code>, <code>/api/send</code>, <code>/api/drafts</code>.
        </p>
      </div>

      <div className="space-y-2">
        <h4 className="font-medium">Tokens activos</h4>
        {tokens.data?.length === 0 && <p className="text-slate-500">No hay tokens.</p>}
        {tokens.data?.map((t) => (
          <div
            key={t.id}
            className="flex items-center gap-3 rounded-lg border border-slate-200 p-2 dark:border-slate-800"
          >
            <Bot size={18} className="shrink-0 text-slate-400" />
            <div className="min-w-0 flex-1">
              <p className="font-medium">
                {t.name} <span className="font-mono text-xs text-slate-400">{t.prefix}…</span>
              </p>
              <p className="text-xs text-slate-500">
                {t.scopes.join(', ')} · {t.accountIds ? `${t.accountIds.length} cuenta(s)` : 'todas las cuentas'} ·{' '}
                {t.lastUsedAt ? `usado ${formatDate(t.lastUsedAt)}` : 'sin usar'}
                {t.expiresAt && ` · caduca ${formatDate(t.expiresAt)}`}
              </p>
            </div>
            <button
              className="btn-ghost text-red-600"
              onClick={() =>
                confirm(`¿Revocar el token "${t.name}"? El agente perderá el acceso.`) && revoke.mutate(t.id)
              }
            >
              Revocar
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ servidor de correo */

function MailServerAdmin() {
  const qc = useQueryClient();
  const status = useQuery({ queryKey: ['ms-status'], queryFn: api.mailserver.status });
  const ready = status.data?.enabled && status.data.ok;
  const domains = useQuery({ queryKey: ['ms-domains'], queryFn: api.mailserver.domains, enabled: !!ready });
  const [newDomain, setNewDomain] = useState('');
  const [selected, setSelected] = useState<string | null>(null);

  const addDomain = useMutation({
    mutationFn: () => api.mailserver.addDomain(newDomain.trim().toLowerCase()),
    onSuccess: (r) => {
      setNewDomain('');
      setSelected(r.domain);
      void qc.invalidateQueries({ queryKey: ['ms-domains'] });
    },
  });

  if (status.isLoading) return <p className="text-sm text-slate-500">Comprobando el servidor…</p>;
  const s = status.data;
  if (!s?.enabled) {
    return (
      <div className="space-y-2 text-sm">
        <p>
          RV Mail está funcionando <b>solo como cliente</b>. Para administrar dominios y buzones, elige un servidor de
          correo en el <code>.env</code> (o con <code>rvmail config</code>):
        </p>
        <p className="text-xs text-slate-500">Mail Server de aaPanel en este VPS:</p>
        <CopyBox
          multiline
          value={'MAILSERVER_PROVIDER=aapanel\nMAILSERVER_HOSTNAME=mail.tudominio.com\nAAPANEL_API_KEY=…'}
        />
        <p className="text-xs text-slate-500">Stalwart (incluido en RV Mail, con JMAP):</p>
        <CopyBox
          multiline
          value={
            'MAILSERVER_PROVIDER=stalwart\nCOMPOSE_PROFILES=stalwart\nMAILSERVER_HOSTNAME=mail.tudominio.com\nSTALWART_ADMIN_USER=admin@tudominio.com\nSTALWART_ADMIN_PASSWORD=…'
          }
        />
        <p className="text-slate-500">
          y reinicia con <code>docker compose up -d</code>. Detalles en <code>docs/MAILSERVER.md</code>.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-5 text-sm">
      <div
        className={`flex items-start gap-2 rounded-xl p-3 ${ready ? 'bg-green-50 dark:bg-green-900/20' : 'bg-amber-50 dark:bg-amber-900/20'}`}
      >
        {ready ? (
          <CheckCircle2 size={18} className="text-green-600" />
        ) : (
          <XCircle size={18} className="text-amber-600" />
        )}
        <div className="flex-1">
          <p className="font-medium">
            {ready
              ? `Conectado a ${s.provider === 'stalwart' ? 'Stalwart' : 'el Mail Server de aaPanel'}`
              : `No se pudo conectar con ${s.provider === 'stalwart' ? 'Stalwart' : 'aaPanel'}`}
          </p>
          <p className="text-xs text-slate-600 dark:text-slate-400">
            Servidor de correo: {s.hostname || '(MAILSERVER_HOSTNAME sin definir)'}
            {s.error && ` · ${s.error}`}
          </p>
          {!ready && s.provider === 'stalwart' && (
            <p className="mt-1 text-xs">
              Revisa <code>STALWART_URL</code> y las credenciales de administrador (<code>STALWART_ADMIN_USER</code>,{' '}
              <code>STALWART_ADMIN_PASSWORD</code>).
            </p>
          )}
          {!ready && s.provider !== 'stalwart' && (
            <p className="mt-1 text-xs">
              Revisa que la API de aaPanel esté activa, que la clave sea correcta y que la IP de la API de RV Mail esté
              en la lista blanca (ver <code>API_IP</code> en <code>.env</code>).
            </p>
          )}
        </div>
        <button className="btn-ghost p-1" onClick={() => void status.refetch()} aria-label="Reintentar">
          <RefreshCw size={16} />
        </button>
      </div>

      {ready && (
        <>
          <div className="space-y-2">
            <h4 className="font-medium">Dominios alojados</h4>
            <div className="flex flex-wrap gap-2">
              {domains.data?.map((d) => (
                <button
                  key={d.domain}
                  onClick={() => setSelected(d.domain)}
                  className={`flex items-center gap-1.5 rounded-full border px-3 py-1 ${
                    selected === d.domain
                      ? 'border-brand-600 bg-brand-50 text-brand-700 dark:bg-brand-700/20 dark:text-brand-100'
                      : 'border-slate-200 dark:border-slate-700'
                  }`}
                >
                  <Globe size={14} /> {d.domain}
                  {d.mailboxes !== null && <span className="text-xs text-slate-400">{d.mailboxes}</span>}
                </button>
              ))}
            </div>
            <form
              className="flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                addDomain.mutate();
              }}
            >
              <input
                className="input"
                placeholder="nuevodominio.com"
                value={newDomain}
                onChange={(e) => setNewDomain(e.target.value)}
              />
              <button className="btn-primary shrink-0" disabled={!newDomain || addDomain.isPending}>
                <Plus size={16} /> Agregar dominio
              </button>
            </form>
            {addDomain.isError && <p className="text-red-600">{(addDomain.error as Error).message}</p>}
          </div>
          {selected && <DomainPanel key={selected} domain={selected} />}
        </>
      )}
    </div>
  );
}

function DomainPanel({ domain }: { domain: string }) {
  const qc = useQueryClient();
  const dns = useQuery({ queryKey: ['ms-dns', domain], queryFn: () => api.mailserver.dns(domain) });
  const boxes = useQuery({ queryKey: ['ms-boxes', domain], queryFn: () => api.mailserver.mailboxes(domain) });
  const [creating, setCreating] = useState(false);

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['ms-boxes', domain] });
    void qc.invalidateQueries({ queryKey: ['ms-domains'] });
  };
  const toggleActive = useMutation({
    mutationFn: (b: { email: string; active: boolean }) => api.mailserver.updateMailbox(b.email, { active: b.active }),
    onSuccess: refresh,
  });
  const resetPassword = useMutation({
    mutationFn: (b: { email: string; password: string }) =>
      api.mailserver.updateMailbox(b.email, { password: b.password }),
  });
  const remove = useMutation({
    mutationFn: (email: string) => api.mailserver.deleteMailbox(email),
    onSuccess: refresh,
  });
  const removeDomain = useMutation({
    mutationFn: () => api.mailserver.deleteDomain(domain),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['ms-domains'] }),
  });

  return (
    <div className="space-y-4 rounded-xl border border-slate-200 p-3 dark:border-slate-800">
      <div className="flex items-center justify-between">
        <h4 className="font-semibold">{domain}</h4>
        <button
          className="btn-ghost text-xs text-red-600"
          onClick={() =>
            confirm(`¿Eliminar el dominio ${domain} del servidor de correo? Sus buzones dejarán de funcionar.`) &&
            removeDomain.mutate()
          }
        >
          Eliminar dominio
        </button>
      </div>

      <div>
        <div className="mb-1 flex items-center justify-between">
          <p className="label mb-0">Registros DNS</p>
          <button className="btn-ghost p-1 text-xs" onClick={() => void dns.refetch()}>
            <RefreshCw size={14} /> Verificar
          </button>
        </div>
        {dns.data ? <DnsTable dns={dns.data} /> : <p className="text-slate-500">Consultando DNS…</p>}
      </div>

      <div>
        <div className="mb-1 flex items-center justify-between">
          <p className="label mb-0">Buzones</p>
          <button className="btn-ghost p-1 text-xs" onClick={() => setCreating(true)}>
            <Plus size={14} /> Nuevo buzón
          </button>
        </div>
        {creating && <NewMailbox domain={domain} onDone={() => (setCreating(false), refresh())} />}
        {resetPassword.isSuccess && <p className="text-xs text-green-700">Contraseña actualizada.</p>}
        {[toggleActive, resetPassword, remove].map(
          (m, i) =>
            m.isError && (
              <p key={i} className="text-xs text-red-600">
                {(m.error as Error).message}
              </p>
            ),
        )}
        <div className="divide-y divide-slate-100 dark:divide-slate-800">
          {boxes.data?.map((b) => (
            <div key={b.email} className="flex flex-wrap items-center gap-2 py-2">
              <div className="min-w-0 flex-1">
                <p className={`truncate ${b.active ? '' : 'text-slate-400 line-through'}`}>{b.email}</p>
                <p className="text-xs text-slate-500">
                  {b.fullName}
                  {b.quotaGb !== null && ` · ${b.quotaGb} GB`}
                </p>
              </div>
              <button
                className="btn-ghost px-2 py-1 text-xs"
                onClick={() => {
                  const pw = prompt(`Nueva contraseña para ${b.email} (mínimo 10 caracteres):`);
                  if (pw) resetPassword.mutate({ email: b.email, password: pw });
                }}
              >
                Contraseña
              </button>
              <button
                className="btn-ghost px-2 py-1 text-xs"
                onClick={() => toggleActive.mutate({ email: b.email, active: !b.active })}
              >
                {b.active ? 'Desactivar' : 'Activar'}
              </button>
              <button
                className="btn-ghost p-1 text-red-600"
                aria-label="Eliminar buzón"
                onClick={() => confirm(`¿Eliminar ${b.email} y todo su correo del servidor?`) && remove.mutate(b.email)}
              >
                <Trash2 size={14} />
              </button>
            </div>
          ))}
          {boxes.data?.length === 0 && <p className="py-2 text-slate-500">Sin buzones.</p>}
        </div>
      </div>
    </div>
  );
}

function DnsTable({ dns }: { dns: DnsCheck }) {
  return (
    <div className="space-y-1">
      {dns.checks.map((c) => (
        <div key={c.record} className="flex items-start gap-2 rounded-lg bg-slate-50 p-2 dark:bg-slate-800/50">
          {c.ok ? (
            <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-green-600" />
          ) : (
            <XCircle size={16} className="mt-0.5 shrink-0 text-amber-600" />
          )}
          <div className="min-w-0 flex-1 text-xs">
            <p className="font-medium">{c.record}</p>
            <p className="break-all text-slate-500">
              {c.found.length ? `Encontrado: ${c.found.join(' · ')}` : 'No encontrado'}
            </p>
            {!c.ok && <p className="break-all text-slate-600 dark:text-slate-300">Debe ser: {c.expected}</p>}
          </div>
        </div>
      ))}
    </div>
  );
}

function NewMailbox({ domain, onDone }: { domain: string; onDone: () => void }) {
  const users = useQuery({ queryKey: ['ms-users'], queryFn: api.mailserver.users });
  const [local, setLocal] = useState('');
  const [fullName, setFullName] = useState('');
  const [password, setPassword] = useState('');
  const [quotaGb, setQuotaGb] = useState(5);
  const [link, setLink] = useState<'none' | 'me' | 'user' | 'new_user'>('new_user');
  const [userId, setUserId] = useState('');
  const [result, setResult] = useState<{ email: string; password?: string } | null>(null);
  const create = useMutation({
    mutationFn: () =>
      api.mailserver.createMailbox({
        email: `${local.trim().toLowerCase()}@${domain}`,
        fullName,
        password: password || undefined,
        quotaGb,
        link,
        userId: link === 'user' ? userId : undefined,
      }),
    onSuccess: setResult,
  });

  if (result) {
    return (
      <div className="mb-2 space-y-2 rounded-lg border border-green-300 bg-green-50 p-3 dark:border-green-800 dark:bg-green-900/20">
        <p className="font-medium">Buzón {result.email} creado.</p>
        {result.password && (
          <>
            <p className="text-xs">Contraseña generada (cópiala ahora, no se volverá a mostrar):</p>
            <CopyBox value={result.password} />
          </>
        )}
        {link === 'new_user' && (
          <p className="text-xs">Esa persona ya puede entrar a RV Mail con su correo y contraseña.</p>
        )}
        <button className="btn-primary" onClick={onDone}>
          Listo
        </button>
      </div>
    );
  }

  return (
    <form
      className="mb-2 space-y-2 rounded-lg bg-slate-50 p-3 dark:bg-slate-800/50"
      onSubmit={(e) => {
        e.preventDefault();
        create.mutate();
      }}
    >
      <div className="grid gap-2 sm:grid-cols-2">
        <div>
          <label className="label">Dirección</label>
          <div className="flex items-center gap-1">
            <input
              className="input"
              value={local}
              onChange={(e) => setLocal(e.target.value)}
              placeholder="ventas"
              required
            />
            <span className="text-xs text-slate-500">@{domain}</span>
          </div>
        </div>
        <div>
          <label className="label">Nombre</label>
          <input className="input" value={fullName} onChange={(e) => setFullName(e.target.value)} required />
        </div>
        <div>
          <label className="label">Contraseña (vacío = generar una segura)</label>
          <input
            className="input"
            type="password"
            autoComplete="new-password"
            minLength={10}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>
        <div>
          <label className="label">Cuota (GB)</label>
          <input
            className="input"
            type="number"
            min={0.1}
            step={0.1}
            value={quotaGb}
            onChange={(e) => setQuotaGb(Number(e.target.value))}
          />
        </div>
      </div>
      <div>
        <label className="label">Acceso en RV Mail</label>
        <select className="input" value={link} onChange={(e) => setLink(e.target.value as typeof link)}>
          <option value="new_user">Crear usuario de RV Mail con este correo y contraseña</option>
          <option value="me">Agregarlo a mis cuentas</option>
          <option value="user">Asignarlo a un usuario existente</option>
          <option value="none">Solo crear el buzón</option>
        </select>
      </div>
      {link === 'user' && (
        <select className="input" value={userId} onChange={(e) => setUserId(e.target.value)} required>
          <option value="">Elige un usuario…</option>
          {users.data?.map((u) => (
            <option key={u.id} value={u.id}>
              {u.name} ({u.email})
            </option>
          ))}
        </select>
      )}
      {create.isError && <p className="text-xs text-red-600">{(create.error as Error).message}</p>}
      <div className="flex justify-end gap-2">
        <button type="button" className="btn-ghost" onClick={onDone}>
          Cancelar
        </button>
        <button className="btn-primary" disabled={create.isPending}>
          {create.isPending ? 'Creando…' : 'Crear buzón'}
        </button>
      </div>
    </form>
  );
}
