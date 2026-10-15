/** Eventos en tiempo real worker → api → navegador (vía Redis pub/sub + SSE). */

export const eventsChannel = (userId: string) => `rvmail:events:${userId}`;

export type AccountStatus = 'pending' | 'syncing' | 'idle' | 'auth_error' | 'error' | 'disabled';

export type RvEvent =
  | { type: 'message.new'; accountId: string; folderId: string; count: number }
  | { type: 'message.updated'; accountId: string; messageIds: string[] }
  | { type: 'folder.updated'; accountId: string; folderId: string }
  | { type: 'account.status'; accountId: string; status: AccountStatus; error?: string | null };

/** Clave de lease que garantiza un solo worker por cuenta. */
export const accountLeaseKey = (accountId: string) => `rvmail:lease:account:${accountId}`;

/** Canal para pedir al worker que (re)sincronice una cuenta de inmediato. */
export const workerControlChannel = 'rvmail:worker:control';

export type WorkerControl =
  | { type: 'account.changed'; accountId: string }
  /** Pide una pasada de sincronización sin reiniciar la sesión (tras enviar, mover, borrar). */
  | { type: 'account.sync'; accountId: string }
  | { type: 'account.deleted'; accountId: string };
