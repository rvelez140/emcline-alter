import { useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { notify } from './native';

/**
 * Escucha /api/events (SSE) y refresca los datos afectados.
 * EventSource reconecta solo si se cae la conexión.
 */
export function useLiveEvents() {
  const qc = useQueryClient();

  useEffect(() => {
    const es = new EventSource('/api/events', { withCredentials: true });
    const refreshMail = () => {
      void qc.invalidateQueries({ queryKey: ['messages'] });
      void qc.invalidateQueries({ queryKey: ['folders'] });
    };

    es.addEventListener('message.new', (ev) => {
      refreshMail();
      const data = JSON.parse((ev as MessageEvent).data) as { count: number };
      if (document.hidden) notify('RV Mail', `${data.count} mensaje(s) nuevo(s)`);
    });
    es.addEventListener('message.updated', refreshMail);
    es.addEventListener('folder.updated', refreshMail);
    es.addEventListener('account.status', () => void qc.invalidateQueries({ queryKey: ['accounts'] }));

    return () => es.close();
  }, [qc]);
}
