import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Menu, PenSquare, Search } from 'lucide-react';
import { useEffect, useState } from 'react';
import { api, MessageFull, Special, takeSsoNotice, User } from '../api';
import { parseMailto } from '../native';
import { useLiveEvents } from '../useLiveEvents';
import { AccountDialog } from './AccountDialog';
import { Compose, ComposeDraft } from './Compose';
import { MessageList } from './MessageList';
import { MessageView } from './MessageView';
import { SettingsDialog } from './SettingsDialog';
import { Sidebar } from './Sidebar';

export type Selection =
  | { kind: 'unified'; special: Special; accountId?: string }
  | { kind: 'folder'; accountId: string; folderId: string; name: string };

type Pane = 'nav' | 'list' | 'read';

export function MailApp({ user }: { user: User }) {
  useLiveEvents();
  const qc = useQueryClient();
  const accounts = useQuery({ queryKey: ['accounts'], queryFn: api.accounts });

  const [selection, setSelection] = useState<Selection>({ kind: 'unified', special: 'inbox' });
  const [messageId, setMessageId] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const [pane, setPane] = useState<Pane>('list');
  const [compose, setCompose] = useState<ComposeDraft | null>(null);
  const [accountDialog, setAccountDialog] = useState<{ id?: string } | null>(null);
  const [settings, setSettings] = useState(false);
  // Aviso al volver de conectar una cuenta o vincular un proveedor (OAuth)
  const [notice, setNotice] = useState(() => takeSsoNotice());

  const hasAccounts = (accounts.data?.length ?? 0) > 0;

  // Enlaces mailto: (manejador del navegador o de la app de escritorio) → redactar
  const firstAccount = accounts.data?.[0]?.id;
  useEffect(() => {
    const raw = new URLSearchParams(location.search).get('mailto');
    const m = raw ? parseMailto(raw) : null;
    if (!m || !firstAccount) return;
    history.replaceState(null, '', '/');
    setCompose({
      accountId: firstAccount,
      to: [m.to, m.cc].filter(Boolean).join(', '),
      subject: m.subject,
      body: m.body,
    });
  }, [firstAccount]);

  function select(s: Selection) {
    setSelection(s);
    setMessageId(null);
    setPane('list');
  }

  function openMessage(id: string) {
    setMessageId(id);
    setPane('read');
  }

  function reply(m: MessageFull, all: boolean) {
    const to = [m.fromAddress, ...(all ? [...m.to, ...m.cc].map((a) => a.address) : [])].filter(
      (a): a is string => !!a && a.toLowerCase() !== m.accountEmail.toLowerCase(),
    );
    setCompose({
      accountId: m.accountId,
      to: [...new Set(to)].join(', '),
      subject: /^re:/i.test(m.subject) ? m.subject : `Re: ${m.subject}`,
      html: `<br><br><blockquote>${m.html ?? (m.text ?? '').replace(/\n/g, '<br>')}</blockquote>`,
      inReplyToMessageId: m.id,
    });
  }

  async function logout() {
    await api.logout();
    qc.clear();
    location.reload();
  }

  return (
    <div className="flex h-full overflow-hidden">
      {/* Panel 1: cuentas y carpetas */}
      <aside
        className={`${pane === 'nav' ? 'flex' : 'hidden'} w-full shrink-0 flex-col border-r border-slate-200 bg-white md:flex md:w-64 dark:border-slate-800 dark:bg-slate-900`}
      >
        <Sidebar
          user={user}
          accounts={accounts.data ?? []}
          selection={selection}
          onSelect={select}
          onCompose={() => setCompose({ accountId: accounts.data?.[0]?.id ?? '' })}
          onAddAccount={() => setAccountDialog({})}
          onEditAccount={(id) => setAccountDialog({ id })}
          onLogout={logout}
          onSettings={() => setSettings(true)}
        />
      </aside>

      {/* Panel 2: lista */}
      <section
        className={`${pane === 'list' ? 'flex' : 'hidden'} w-full min-w-0 flex-col border-r border-slate-200 md:flex md:w-96 lg:w-[28rem] dark:border-slate-800`}
      >
        <div className="flex items-center gap-2 border-b border-slate-200 p-2 dark:border-slate-800">
          <button className="btn-ghost md:hidden" onClick={() => setPane('nav')} aria-label="Carpetas">
            <Menu size={18} />
          </button>
          <div className="relative flex-1">
            <Search size={16} className="absolute top-2.5 left-2.5 text-slate-400" />
            <input
              className="input pl-8"
              placeholder="Buscar en el correo"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <button
            className="btn-primary md:hidden"
            onClick={() => setCompose({ accountId: accounts.data?.[0]?.id ?? '' })}
            aria-label="Redactar"
          >
            <PenSquare size={18} />
          </button>
        </div>
        {hasAccounts ? (
          <MessageList selection={selection} search={search} selectedId={messageId} onOpen={openMessage} />
        ) : (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center text-sm text-slate-500">
            <p>Aún no has agregado ninguna cuenta de correo.</p>
            <button className="btn-primary" onClick={() => setAccountDialog({})}>
              Agregar cuenta
            </button>
          </div>
        )}
      </section>

      {/* Panel 3: lectura */}
      <main className={`${pane === 'read' ? 'flex' : 'hidden'} min-w-0 flex-1 flex-col md:flex`}>
        {messageId ? (
          <MessageView
            id={messageId}
            onBack={() => setPane('list')}
            onReply={reply}
            onForward={(m) =>
              setCompose({
                accountId: m.accountId,
                subject: /^(fwd?|rv):/i.test(m.subject) ? m.subject : `Rv: ${m.subject}`,
                html: `<br><br>---------- Mensaje reenviado ----------<br>${m.html ?? (m.text ?? '').replace(/\n/g, '<br>')}`,
              })
            }
            onDeleted={() => {
              setMessageId(null);
              setPane('list');
            }}
          />
        ) : (
          <div className="flex flex-1 items-center justify-center text-sm text-slate-400">
            Selecciona un mensaje para leerlo
          </div>
        )}
      </main>

      {compose && <Compose accounts={accounts.data ?? []} draft={compose} onClose={() => setCompose(null)} />}
      {notice && (
        <div
          role="status"
          className={`fixed bottom-4 left-1/2 z-50 flex -translate-x-1/2 items-center gap-3 rounded-xl px-4 py-3 text-sm shadow-lg ${
            notice.error ? 'bg-red-600 text-white' : 'bg-slate-900 text-white dark:bg-slate-100 dark:text-slate-900'
          }`}
        >
          {notice.error ?? notice.ok}
          <button className="opacity-70 hover:opacity-100" onClick={() => setNotice(null)} aria-label="Cerrar aviso">
            ✕
          </button>
        </div>
      )}
      {settings && (
        <SettingsDialog
          user={user}
          accounts={accounts.data ?? []}
          onClose={() => {
            setSettings(false);
            // Un buzón nuevo puede haberse agregado a las cuentas del usuario
            void qc.invalidateQueries({ queryKey: ['accounts'] });
          }}
        />
      )}
      {accountDialog && (
        <AccountDialog
          account={accounts.data?.find((a) => a.id === accountDialog.id)}
          onClose={() => setAccountDialog(null)}
        />
      )}
    </div>
  );
}
