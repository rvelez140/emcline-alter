import { useInfiniteQuery } from '@tanstack/react-query';
import { Paperclip, Star } from 'lucide-react';
import { useDeferredValue } from 'react';
import { api, formatDate } from '../api';
import type { Selection } from './MailApp';

type Props = {
  selection: Selection;
  search: string;
  selectedId: string | null;
  onOpen: (id: string) => void;
};

export function MessageList({ selection, search, selectedId, onOpen }: Props) {
  const q = useDeferredValue(search.trim());
  const params =
    selection.kind === 'unified'
      ? { special: selection.special, accountId: selection.accountId, q }
      : { folderId: selection.folderId, q };

  const query = useInfiniteQuery({
    queryKey: ['messages', params],
    queryFn: ({ pageParam }) => api.messages({ ...params, cursor: pageParam }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.nextCursor ?? undefined,
  });

  const items = query.data?.pages.flatMap((p) => p.items) ?? [];

  if (query.isLoading) return <p className="p-4 text-sm text-slate-500">Cargando mensajes…</p>;
  if (query.isError) return <p className="p-4 text-sm text-red-600">{(query.error as Error).message}</p>;
  if (!items.length) {
    return (
      <p className="p-6 text-center text-sm text-slate-500">
        {q ? 'No hay resultados para tu búsqueda.' : 'No hay mensajes aquí. Si acabas de agregar la cuenta, la sincronización puede tardar unos minutos.'}
      </p>
    );
  }

  return (
    <ul
      className="flex-1 overflow-y-auto"
      onScroll={(e) => {
        const el = e.currentTarget;
        if (el.scrollTop + el.clientHeight > el.scrollHeight - 300 && query.hasNextPage && !query.isFetchingNextPage) {
          void query.fetchNextPage();
        }
      }}
    >
      {items.map((m) => (
        <li key={m.id}>
          <button
            onClick={() => onOpen(m.id)}
            className={`flex w-full gap-3 border-b border-slate-100 px-3 py-2.5 text-left dark:border-slate-800/70 ${
              selectedId === m.id ? 'bg-brand-50 dark:bg-brand-700/20' : 'hover:bg-slate-100 dark:hover:bg-slate-900'
            }`}
          >
            <span className="mt-1.5 h-2 w-2 shrink-0 rounded-full" style={{ background: m.accountColor }} title={m.accountEmail} />
            <div className="min-w-0 flex-1">
              <div className="flex items-baseline gap-2">
                <span className={`flex-1 truncate text-sm ${m.seen ? '' : 'font-semibold'}`}>
                  {m.fromName || m.fromAddress || '(sin remitente)'}
                </span>
                {m.hasAttachments && <Paperclip size={12} className="shrink-0 text-slate-400" />}
                {m.flagged && <Star size={12} className="shrink-0 fill-amber-400 text-amber-400" />}
                <span className="shrink-0 text-xs text-slate-500">{formatDate(m.date)}</span>
              </div>
              <p className={`truncate text-sm ${m.seen ? 'text-slate-600 dark:text-slate-400' : 'font-medium'}`}>
                {m.subject || '(sin asunto)'}
              </p>
              <p className="truncate text-xs text-slate-500">{m.snippet}</p>
            </div>
          </button>
        </li>
      ))}
      {query.isFetchingNextPage && <li className="p-3 text-center text-xs text-slate-500">Cargando más…</li>}
    </ul>
  );
}
