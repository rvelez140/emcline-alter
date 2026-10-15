import { useQuery } from '@tanstack/react-query';
import {
  AlertTriangle,
  Archive,
  Folder as FolderIcon,
  Inbox,
  LogOut,
  PenSquare,
  Plus,
  Send,
  Settings,
  ShieldAlert,
  Trash2,
} from 'lucide-react';
import { ReactNode, useEffect } from 'react';
import { Account, api, Folder, Special, User } from '../api';
import { setUnread } from '../native';
import type { Selection } from './MailApp';

const UNIFIED: Array<{ special: Special; label: string; icon: ReactNode; use: string }> = [
  { special: 'inbox', label: 'Bandeja de entrada', icon: <Inbox size={16} />, use: '\\Inbox' },
  { special: 'sent', label: 'Enviados', icon: <Send size={16} />, use: '\\Sent' },
  { special: 'archive', label: 'Archivo', icon: <Archive size={16} />, use: '\\Archive' },
  { special: 'junk', label: 'Spam', icon: <ShieldAlert size={16} />, use: '\\Junk' },
  { special: 'trash', label: 'Papelera', icon: <Trash2 size={16} />, use: '\\Trash' },
];

type Props = {
  user: User;
  accounts: Account[];
  selection: Selection;
  onSelect: (s: Selection) => void;
  onCompose: () => void;
  onAddAccount: () => void;
  onEditAccount: (id: string) => void;
  onLogout: () => void;
  onSettings: () => void;
};

export function Sidebar({
  user,
  accounts,
  selection,
  onSelect,
  onCompose,
  onAddAccount,
  onEditAccount,
  onLogout,
  onSettings,
}: Props) {
  const folders = useQuery({ queryKey: ['folders'], queryFn: api.folders, enabled: accounts.length > 0 });
  const all = folders.data ?? [];

  const unreadFor = (use: string) => all.filter((f) => f.specialUse === use).reduce((n, f) => n + f.unreadCount, 0);
  const inboxUnread = unreadFor('\\Inbox');
  // Contador en la bandeja del sistema (app de escritorio)
  useEffect(() => setUnread(inboxUnread), [inboxUnread]);

  const isUnified = (s: Special) => selection.kind === 'unified' && selection.special === s && !selection.accountId;

  return (
    <div className="flex h-full flex-col">
      <div className="p-3">
        <button className="btn-primary w-full" onClick={onCompose} disabled={accounts.length === 0}>
          <PenSquare size={16} /> Redactar
        </button>
      </div>

      <nav className="flex-1 overflow-y-auto px-2 pb-4 text-sm">
        <SectionTitle>Todas las cuentas</SectionTitle>
        {UNIFIED.map((u) => (
          <NavItem
            key={u.special}
            icon={u.icon}
            label={u.label}
            count={u.special === 'inbox' ? unreadFor(u.use) : 0}
            active={isUnified(u.special)}
            onClick={() => onSelect({ kind: 'unified', special: u.special })}
          />
        ))}

        {accounts.map((acc) => (
          <AccountSection
            key={acc.id}
            account={acc}
            folders={all.filter((f) => f.accountId === acc.id)}
            selection={selection}
            onSelect={onSelect}
            onEdit={() => onEditAccount(acc.id)}
          />
        ))}

        <button className="btn-ghost mt-3 w-full justify-start" onClick={onAddAccount}>
          <Plus size={16} /> Agregar cuenta
        </button>
      </nav>

      <div className="flex items-center justify-between border-t border-slate-200 p-3 text-xs dark:border-slate-800">
        <div className="min-w-0">
          <p className="truncate font-medium">{user.name}</p>
          <p className="truncate text-slate-500">{user.email}</p>
        </div>
        <div className="flex shrink-0">
          <button className="btn-ghost" onClick={onSettings} aria-label="Configuración" title="Configuración">
            <Settings size={16} />
          </button>
          <button className="btn-ghost" onClick={onLogout} aria-label="Cerrar sesión" title="Cerrar sesión">
            <LogOut size={16} />
          </button>
        </div>
      </div>
    </div>
  );
}

function AccountSection({
  account,
  folders,
  selection,
  onSelect,
  onEdit,
}: {
  account: Account;
  folders: Folder[];
  selection: Selection;
  onSelect: (s: Selection) => void;
  onEdit: () => void;
}) {
  const sorted = [...folders].sort((a, b) => rank(a) - rank(b) || a.path.localeCompare(b.path));
  const hasProblem = account.status === 'auth_error' || account.status === 'error';

  return (
    <div className="mt-4">
      <div className="group flex items-center gap-2 px-2 py-1">
        <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: account.color }} />
        <span className="flex-1 truncate text-xs font-semibold text-slate-500 uppercase" title={account.email}>
          {account.displayName}
        </span>
        {hasProblem && (
          <span title={account.lastError ?? 'Error'}>
            <AlertTriangle size={14} className="text-amber-500" />
          </span>
        )}
        {account.status === 'syncing' && <span className="text-[10px] text-slate-400">sincronizando…</span>}
        <button className="opacity-60 hover:opacity-100" onClick={onEdit} aria-label="Configurar cuenta">
          <Settings size={14} />
        </button>
      </div>
      {sorted.map((f) => (
        <NavItem
          key={f.id}
          icon={iconFor(f.specialUse)}
          label={labelFor(f)}
          count={f.unreadCount}
          active={selection.kind === 'folder' && selection.folderId === f.id}
          onClick={() => onSelect({ kind: 'folder', accountId: account.id, folderId: f.id, name: labelFor(f) })}
        />
      ))}
    </div>
  );
}

function SectionTitle({ children }: { children: ReactNode }) {
  return <p className="px-2 pt-1 pb-1 text-xs font-semibold text-slate-500 uppercase">{children}</p>;
}

function NavItem(props: { icon: ReactNode; label: string; count: number; active: boolean; onClick: () => void }) {
  return (
    <button
      onClick={props.onClick}
      className={`flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left ${
        props.active
          ? 'bg-brand-50 font-medium text-brand-700 dark:bg-brand-700/20 dark:text-brand-100'
          : 'hover:bg-slate-100 dark:hover:bg-slate-800'
      }`}
    >
      {props.icon}
      <span className="flex-1 truncate">{props.label}</span>
      {props.count > 0 && <span className="text-xs font-semibold">{props.count}</span>}
    </button>
  );
}

const ORDER = ['\\Inbox', '\\Drafts', '\\Sent', '\\Archive', '\\Junk', '\\Trash'];
function rank(f: Folder) {
  const i = ORDER.indexOf(f.specialUse ?? '');
  return i === -1 ? 50 : i;
}

function iconFor(use: string | null) {
  switch (use) {
    case '\\Inbox':
      return <Inbox size={16} />;
    case '\\Sent':
      return <Send size={16} />;
    case '\\Trash':
      return <Trash2 size={16} />;
    case '\\Junk':
      return <ShieldAlert size={16} />;
    case '\\Archive':
      return <Archive size={16} />;
    default:
      return <FolderIcon size={16} />;
  }
}

const LABELS: Record<string, string> = {
  '\\Inbox': 'Bandeja de entrada',
  '\\Sent': 'Enviados',
  '\\Drafts': 'Borradores',
  '\\Trash': 'Papelera',
  '\\Junk': 'Spam',
  '\\Archive': 'Archivo',
};
function labelFor(f: Folder) {
  return (f.specialUse && LABELS[f.specialUse]) || f.name;
}
