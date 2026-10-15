import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { FormEvent, useState } from 'react';
import { Account, api, ssoStart } from '../api';
import { SsoButtons } from './SsoButtons';

/**
 * Proveedores comunes. El autodescubrimiento completo (autoconfig/SRV/MX)
 * y OAuth para Gmail/Microsoft están en el roadmap.
 */
const PRESETS: Record<
  string,
  {
    label: string;
    imap: [string, number, boolean];
    smtp: [string, number, boolean];
    hint?: string;
  }
> = {
  custom: {
    label: 'Otro / servidor propio',
    imap: ['', 993, true],
    smtp: ['', 465, true],
  },
  gmail: {
    label: 'Gmail',
    imap: ['imap.gmail.com', 993, true],
    smtp: ['smtp.gmail.com', 465, true],
    hint: 'Gmail requiere una "contraseña de aplicación" (Cuenta de Google → Seguridad → Verificación en 2 pasos).',
  },
  outlook: {
    label: 'Outlook / Hotmail',
    imap: ['outlook.office365.com', 993, true],
    smtp: ['smtp.office365.com', 587, false],
    hint: 'Microsoft está retirando la autenticación básica; para cuentas de M365 se usará OAuth (roadmap).',
  },
  yahoo: {
    label: 'Yahoo',
    imap: ['imap.mail.yahoo.com', 993, true],
    smtp: ['smtp.mail.yahoo.com', 465, true],
  },
  icloud: {
    label: 'iCloud',
    imap: ['imap.mail.me.com', 993, true],
    smtp: ['smtp.mail.me.com', 587, false],
  },
};

const COLORS = ['#2563eb', '#16a34a', '#dc2626', '#9333ea', '#ea580c', '#0891b2', '#db2777', '#4b5563'];

export function AccountDialog({ account, onClose }: { account?: Account; onClose: () => void }) {
  const qc = useQueryClient();
  const editing = !!account;
  const isOAuth = account?.authType === 'oauth2';
  // Gmail / Outlook / Yahoo sin contraseñas de aplicación (si el administrador configuró OAuth)
  const oauth = useQuery({
    queryKey: ['oauth-providers'],
    queryFn: api.oauthProviders,
  });
  const [preset, setPreset] = useState('custom');
  const [f, setF] = useState({
    displayName: account?.displayName ?? '',
    email: account?.email ?? '',
    username: account?.username ?? '',
    password: '',
    imapHost: account?.imapHost ?? '',
    imapPort: account?.imapPort ?? 993,
    imapSecure: account?.imapSecure ?? true,
    smtpHost: account?.smtpHost ?? '',
    smtpPort: account?.smtpPort ?? 465,
    smtpSecure: account?.smtpSecure ?? true,
    color: account?.color ?? COLORS[0],
  });
  const set = <K extends keyof typeof f>(k: K, v: (typeof f)[K]) => setF((s) => ({ ...s, [k]: v }));

  function applyPreset(key: string) {
    setPreset(key);
    const p = PRESETS[key];
    if (key === 'custom') return;
    setF((s) => ({
      ...s,
      imapHost: p.imap[0],
      imapPort: p.imap[1],
      imapSecure: p.imap[2],
      smtpHost: p.smtp[0],
      smtpPort: p.smtp[1],
      smtpSecure: p.smtp[2],
    }));
  }

  function onEmail(email: string) {
    set('email', email);
    if (!editing && (!f.username || f.username === f.email)) set('username', email);
    if (!editing && preset === 'custom' && !f.imapHost) {
      const domain = email.split('@')[1];
      if (domain && domain.includes('.')) {
        setF((s) => ({
          ...s,
          email,
          username: email,
          imapHost: `mail.${domain}`,
          smtpHost: `mail.${domain}`,
        }));
      }
    }
  }

  const [managed, setManaged] = useState(false);

  /** Si el dominio está alojado en el servidor de correo integrado, se rellena todo solo. */
  async function detectServer() {
    if (editing || !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(f.email)) return;
    const cfg = await api.autoconfig(f.email).catch(() => null);
    if (!cfg?.managed) {
      setManaged(false);
      return;
    }
    setManaged(true);
    setPreset('custom');
    setF((s) => ({
      ...s,
      username: s.email,
      imapHost: cfg.imapHost,
      imapPort: cfg.imapPort,
      imapSecure: cfg.imapSecure,
      smtpHost: cfg.smtpHost,
      smtpPort: cfg.smtpPort,
      smtpSecure: cfg.smtpSecure,
    }));
  }

  const save = useMutation({
    mutationFn: () => {
      const { password, ...rest } = f;
      return editing
        ? api.updateAccount(account!.id, password ? { ...rest, password } : rest)
        : api.createAccount({ ...rest, password });
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['accounts'] });
      void qc.invalidateQueries({ queryKey: ['folders'] });
      onClose();
    },
  });

  const remove = useMutation({
    mutationFn: () => api.deleteAccount(account!.id),
    onSuccess: () => {
      void qc.invalidateQueries();
      onClose();
    },
  });

  function submit(e: FormEvent) {
    e.preventDefault();
    save.mutate();
  }

  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/30 md:items-center md:p-6">
      <form
        onSubmit={submit}
        className="flex max-h-full w-full flex-col overflow-hidden bg-white shadow-2xl md:max-w-xl md:rounded-2xl dark:bg-slate-900"
      >
        <div className="flex items-center justify-between border-b border-slate-200 px-4 py-3 dark:border-slate-800">
          <h3 className="font-semibold">{editing ? 'Configurar cuenta' : 'Agregar cuenta de correo'}</h3>
          <button type="button" className="btn-ghost" onClick={onClose} aria-label="Cerrar">
            <X size={18} />
          </button>
        </div>

        <div className="space-y-4 overflow-y-auto p-4">
          {!editing && (oauth.data?.length ?? 0) > 0 && (
            <div className="space-y-2 rounded-xl bg-slate-50 p-3 dark:bg-slate-800/50">
              <p className="text-xs text-slate-600 dark:text-slate-400">
                Recomendado para Gmail, Outlook/Microsoft 365 y Yahoo: sin contraseñas de aplicación.
              </p>
              <SsoButtons providers={oauth.data ?? []} mode="account" label={(n) => `Conectar con ${n}`} />
              <p className="pt-1 text-center text-xs text-slate-400">o configura la cuenta manualmente:</p>
            </div>
          )}

          {isOAuth && (
            <div className="space-y-2 rounded-xl bg-slate-50 p-3 text-xs dark:bg-slate-800/50">
              <p>
                Esta cuenta está conectada con <b>OAuth</b> ({account?.imapHost}
                ). No guarda tu contraseña.
              </p>
              {account?.status === 'auth_error' && (
                <p className="text-amber-700 dark:text-amber-300">
                  El proveedor revocó el acceso: vuelve a conectarla.
                </p>
              )}
              {(oauth.data ?? [])
                .filter((p) =>
                  account?.imapHost.includes(
                    p.id === 'microsoft' ? 'office365' : p.id === 'google' ? 'gmail' : 'yahoo',
                  ),
                )
                .map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    className="btn-ghost border border-slate-300 dark:border-slate-700"
                    onClick={() => ssoStart(p.id, 'account')}
                  >
                    Reconectar con {p.name}
                  </button>
                ))}
            </div>
          )}

          {!editing && (
            <div>
              <label className="label">Proveedor</label>
              <select className="input" value={preset} onChange={(e) => applyPreset(e.target.value)}>
                {Object.entries(PRESETS).map(([k, p]) => (
                  <option key={k} value={k}>
                    {p.label}
                  </option>
                ))}
              </select>
              {PRESETS[preset].hint && <p className="mt-1 text-xs text-amber-600">{PRESETS[preset].hint}</p>}
            </div>
          )}

          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Nombre a mostrar">
              <input
                className="input"
                value={f.displayName}
                onChange={(e) => set('displayName', e.target.value)}
                required
              />
            </Field>
            <Field label="Correo">
              <input
                className="input"
                type="email"
                value={f.email}
                onChange={(e) => onEmail(e.target.value)}
                onBlur={() => void detectServer()}
                required
              />
            </Field>
            {!isOAuth && (
              <Field label="Usuario">
                <input
                  className="input"
                  value={f.username}
                  onChange={(e) => set('username', e.target.value)}
                  required
                />
              </Field>
            )}
            {!isOAuth && (
              <Field label={editing ? 'Contraseña (vacío = no cambiar)' : 'Contraseña'}>
                <input
                  className="input"
                  type="password"
                  autoComplete="new-password"
                  value={f.password}
                  onChange={(e) => set('password', e.target.value)}
                  required={!editing}
                />
              </Field>
            )}
          </div>

          {managed && (
            <p className="rounded-lg bg-green-50 p-2 text-xs text-green-800 dark:bg-green-900/30 dark:text-green-200">
              Este dominio está alojado en tu servidor de correo: la configuración se completó sola. Solo falta la
              contraseña.
            </p>
          )}

          {!isOAuth && (
            <>
              <fieldset className="grid gap-3 sm:grid-cols-[1fr_6rem_auto]">
                <legend className="label">Entrante (IMAP)</legend>
                <input
                  className="input"
                  placeholder="imap.dominio.com"
                  value={f.imapHost}
                  onChange={(e) => set('imapHost', e.target.value)}
                  required
                />
                <input
                  className="input"
                  type="number"
                  value={f.imapPort}
                  onChange={(e) => set('imapPort', Number(e.target.value))}
                  required
                />
                <label className="flex items-center gap-2 text-sm">
                  <input type="checkbox" checked={f.imapSecure} onChange={(e) => set('imapSecure', e.target.checked)} />{' '}
                  SSL/TLS
                </label>
              </fieldset>

              <fieldset className="grid gap-3 sm:grid-cols-[1fr_6rem_auto]">
                <legend className="label">Saliente (SMTP)</legend>
                <input
                  className="input"
                  placeholder="smtp.dominio.com"
                  value={f.smtpHost}
                  onChange={(e) => set('smtpHost', e.target.value)}
                  required
                />
                <input
                  className="input"
                  type="number"
                  value={f.smtpPort}
                  onChange={(e) => set('smtpPort', Number(e.target.value))}
                  required
                />
                <label className="flex items-center gap-2 text-sm">
                  <input type="checkbox" checked={f.smtpSecure} onChange={(e) => set('smtpSecure', e.target.checked)} />{' '}
                  SSL/TLS
                </label>
              </fieldset>
              <p className="text-xs text-slate-500">
                Puerto 465 = SSL/TLS activado. Puerto 587 = STARTTLS (deja SSL/TLS desactivado).
              </p>
            </>
          )}

          <div>
            <label className="label">Color de la cuenta</label>
            <div className="flex gap-2">
              {COLORS.map((c) => (
                <button
                  type="button"
                  key={c}
                  onClick={() => set('color', c)}
                  className={`h-7 w-7 rounded-full ${f.color === c ? 'ring-2 ring-offset-2 ring-slate-400' : ''}`}
                  style={{ background: c }}
                  aria-label={`Color ${c}`}
                />
              ))}
            </div>
          </div>

          {account?.lastError && (
            <p className="rounded-lg bg-amber-50 p-2 text-xs text-amber-800 dark:bg-amber-900/30 dark:text-amber-200">
              Último error: {account.lastError}
            </p>
          )}
          {save.isError && <p className="text-sm text-red-600">{(save.error as Error).message}</p>}
        </div>

        <div className="flex items-center gap-2 border-t border-slate-200 p-3 dark:border-slate-800">
          {editing && (
            <button
              type="button"
              className="btn-ghost text-red-600"
              onClick={() =>
                confirm('¿Quitar esta cuenta de RV Mail? No se borra nada del servidor de correo.') && remove.mutate()
              }
            >
              Quitar cuenta
            </button>
          )}
          <div className="flex-1" />
          <button type="button" className="btn-ghost" onClick={onClose}>
            Cancelar
          </button>
          <button className="btn-primary" disabled={save.isPending}>
            {save.isPending ? 'Probando conexión…' : 'Guardar'}
          </button>
        </div>
      </form>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <label className="label">{label}</label>
      {children}
    </div>
  );
}
