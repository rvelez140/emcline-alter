import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import DOMPurify from 'dompurify';
import { ArrowLeft, Forward, ImageOff, Paperclip, Reply, ReplyAll, Star, Trash2 } from 'lucide-react';
import { useMemo, useState } from 'react';
import { api, MessageFull } from '../api';

type Props = {
  id: string;
  onBack: () => void;
  onReply: (m: MessageFull, all: boolean) => void;
  onForward: (m: MessageFull) => void;
  onDeleted: () => void;
};

export function MessageView({ id, onBack, onReply, onForward, onDeleted }: Props) {
  const qc = useQueryClient();
  const [showImages, setShowImages] = useState(false);
  const msg = useQuery({ queryKey: ['message', id], queryFn: () => api.message(id), staleTime: 5 * 60_000 });

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['messages'] });
    void qc.invalidateQueries({ queryKey: ['folders'] });
  };
  const flag = useMutation({
    mutationFn: (flagged: boolean) => api.patchMessage(id, { flagged }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['message', id] });
      refresh();
    },
  });
  const del = useMutation({
    mutationFn: () => api.deleteMessage(id),
    onSuccess: () => {
      refresh();
      onDeleted();
    },
  });

  const m = msg.data;
  const { html, blockedImages } = useMemo(() => sanitize(m, showImages), [m, showImages]);

  if (msg.isLoading) return <p className="p-6 text-sm text-slate-500">Abriendo mensaje…</p>;
  if (msg.isError || !m) return <p className="p-6 text-sm text-red-600">{(msg.error as Error)?.message}</p>;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-1 border-b border-slate-200 p-2 dark:border-slate-800">
        <button className="btn-ghost md:hidden" onClick={onBack} aria-label="Volver">
          <ArrowLeft size={18} />
        </button>
        <button className="btn-ghost" onClick={() => onReply(m, false)} title="Responder">
          <Reply size={18} />
        </button>
        <button className="btn-ghost" onClick={() => onReply(m, true)} title="Responder a todos">
          <ReplyAll size={18} />
        </button>
        <button className="btn-ghost" onClick={() => onForward(m)} title="Reenviar">
          <Forward size={18} />
        </button>
        <div className="flex-1" />
        <button className="btn-ghost" onClick={() => flag.mutate(!m.flagged)} title="Destacar">
          <Star size={18} className={m.flagged ? 'fill-amber-400 text-amber-400' : ''} />
        </button>
        <button className="btn-ghost" onClick={() => del.mutate()} disabled={del.isPending} title="Eliminar">
          <Trash2 size={18} />
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <header className="space-y-1 px-5 pt-4 pb-3">
          <h2 className="text-lg font-semibold">{m.subject || '(sin asunto)'}</h2>
          <div className="flex flex-wrap items-center gap-x-2 text-sm">
            <span className="font-medium">{m.fromName || m.fromAddress}</span>
            {m.fromName && <span className="text-slate-500">&lt;{m.fromAddress}&gt;</span>}
          </div>
          <p className="text-xs text-slate-500">
            Para: {m.to.map((a) => a.name || a.address).join(', ')}
            {m.cc.length > 0 && <> · CC: {m.cc.map((a) => a.name || a.address).join(', ')}</>}
          </p>
          <p className="flex items-center gap-2 text-xs text-slate-500">
            <span className="h-2 w-2 rounded-full" style={{ background: m.accountColor }} />
            {m.accountEmail} · {new Date(m.date).toLocaleString('es-DO')}
          </p>
        </header>

        {blockedImages && (
          <div className="mx-5 mb-2 flex items-center gap-2 rounded-lg bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:bg-amber-900/30 dark:text-amber-200">
            <ImageOff size={14} /> Imágenes remotas bloqueadas para proteger tu privacidad.
            <button className="font-semibold underline" onClick={() => setShowImages(true)}>
              Mostrar
            </button>
          </div>
        )}

        {m.attachments.length > 0 && (
          <div className="mx-5 mb-3 flex flex-wrap gap-2">
            {m.attachments.map((a, i) => (
              <span
                key={i}
                className="inline-flex items-center gap-1 rounded-lg border border-slate-200 px-2 py-1 text-xs dark:border-slate-700"
              >
                <Paperclip size={12} /> {a.filename ?? 'adjunto'} · {Math.ceil(a.size / 1024)} KB
              </span>
            ))}
          </div>
        )}

        {/* El HTML del correo se aísla: sin scripts, sin acceso al origen de la app */}
        <iframe
          title="Contenido del mensaje"
          sandbox="allow-popups allow-popups-to-escape-sandbox"
          className="h-[70vh] w-full border-0 bg-white"
          srcDoc={html}
        />
      </div>
    </div>
  );
}

function sanitize(m: MessageFull | undefined, allowImages: boolean): { html: string; blockedImages: boolean } {
  if (!m) return { html: '', blockedImages: false };
  const source = m.html ?? `<pre style="white-space:pre-wrap;font-family:inherit">${escapeHtml(m.text ?? '')}</pre>`;
  let blocked = false;

  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (node.tagName === 'A') {
      node.setAttribute('target', '_blank');
      node.setAttribute('rel', 'noopener noreferrer');
    }
    if (!allowImages && node.tagName === 'IMG') {
      const src = node.getAttribute('src') ?? '';
      if (/^https?:/i.test(src)) {
        node.setAttribute('data-blocked-src', src);
        node.removeAttribute('src');
        blocked = true;
      }
    }
  });
  const clean = DOMPurify.sanitize(source, { WHOLE_DOCUMENT: true, FORBID_TAGS: ['script', 'form', 'iframe'] });
  DOMPurify.removeHook('afterSanitizeAttributes');

  const base = `<base target="_blank"><style>body{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;font-size:14px;color:#0f172a;margin:16px 20px;word-wrap:break-word}img{max-width:100%;height:auto}blockquote{border-left:3px solid #cbd5e1;margin:0;padding-left:12px;color:#475569}</style>`;
  return { html: base + clean, blockedImages: blocked };
}

function escapeHtml(s: string) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}
