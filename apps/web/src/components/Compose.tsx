import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Send, X } from 'lucide-react';
import { useState } from 'react';
import { Account, api, splitAddresses } from '../api';

export type ComposeDraft = {
  accountId: string;
  to?: string;
  subject?: string;
  html?: string;
  inReplyToMessageId?: string;
};

/**
 * Redacción básica. El editor enriquecido (TipTap), adjuntos y borradores
 * están en el roadmap (Fase 1).
 */
export function Compose({
  accounts,
  draft,
  onClose,
}: {
  accounts: Account[];
  draft: ComposeDraft;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const [accountId, setAccountId] = useState(draft.accountId || accounts[0]?.id || '');
  const [to, setTo] = useState(draft.to ?? '');
  const [cc, setCc] = useState('');
  const [bcc, setBcc] = useState('');
  const [showCc, setShowCc] = useState(false);
  const [subject, setSubject] = useState(draft.subject ?? '');
  const [body, setBody] = useState('');

  const send = useMutation({
    mutationFn: () =>
      api.send({
        accountId,
        to: splitAddresses(to),
        cc: splitAddresses(cc),
        bcc: splitAddresses(bcc),
        subject,
        html: `<div>${escapeHtml(body).replace(/\n/g, '<br>')}</div>${draft.html ?? ''}`,
        inReplyToMessageId: draft.inReplyToMessageId,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['messages'] });
      onClose();
    },
  });

  return (
    <div className="fixed inset-0 z-40 flex items-end justify-center bg-black/30 md:items-center md:p-6">
      <div className="flex h-full w-full flex-col bg-white shadow-2xl md:h-[80vh] md:max-w-2xl md:rounded-2xl dark:bg-slate-900">
        <div className="flex items-center justify-between border-b border-slate-200 px-4 py-2 dark:border-slate-800">
          <h3 className="font-semibold">{draft.inReplyToMessageId ? 'Responder' : 'Nuevo mensaje'}</h3>
          <button className="btn-ghost" onClick={onClose} aria-label="Cerrar">
            <X size={18} />
          </button>
        </div>

        <div className="space-y-2 border-b border-slate-200 p-4 dark:border-slate-800">
          <Row label="De">
            <select className="input" value={accountId} onChange={(e) => setAccountId(e.target.value)}>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.displayName} &lt;{a.email}&gt;
                </option>
              ))}
            </select>
          </Row>
          <Row label="Para">
            <div className="flex gap-2">
              <input
                className="input"
                value={to}
                onChange={(e) => setTo(e.target.value)}
                placeholder="correo@ejemplo.com"
              />
              {!showCc && (
                <button className="btn-ghost text-xs" onClick={() => setShowCc(true)}>
                  CC/CCO
                </button>
              )}
            </div>
          </Row>
          {showCc && (
            <>
              <Row label="CC">
                <input className="input" value={cc} onChange={(e) => setCc(e.target.value)} />
              </Row>
              <Row label="CCO">
                <input className="input" value={bcc} onChange={(e) => setBcc(e.target.value)} />
              </Row>
            </>
          )}
          <Row label="Asunto">
            <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} />
          </Row>
        </div>

        <textarea
          className="min-h-0 flex-1 resize-none bg-transparent p-4 text-sm outline-none"
          placeholder="Escribe tu mensaje…"
          value={body}
          onChange={(e) => setBody(e.target.value)}
          autoFocus
        />

        <div className="flex items-center gap-3 border-t border-slate-200 p-3 dark:border-slate-800">
          <button
            className="btn-primary"
            onClick={() => send.mutate()}
            disabled={send.isPending || !accountId || splitAddresses(to).length === 0}
          >
            <Send size={16} /> {send.isPending ? 'Enviando…' : 'Enviar'}
          </button>
          {send.isError && <p className="text-sm text-red-600">{(send.error as Error).message}</p>}
        </div>
      </div>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-3">
      <span className="w-14 shrink-0 text-xs text-slate-500">{label}</span>
      <div className="flex-1">{children}</div>
    </div>
  );
}

function escapeHtml(s: string) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}
