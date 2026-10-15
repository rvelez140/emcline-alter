import { createHash } from 'node:crypto';
import http from 'node:http';
import https from 'node:https';

/**
 * Cliente mínimo de la API de aaPanel (Settings → API interface).
 *
 * Firma de cada petición (documentación oficial de la API de aaPanel):
 *   request_time  = timestamp Unix
 *   request_token = md5(request_time + md5(api_sk))
 * La IP desde la que llega la API (la del contenedor, vista por el host como la
 * puerta de enlace de la red Docker) debe estar en la lista blanca del panel.
 */
export class AaPanelClient {
  constructor(
    private readonly baseUrl: string,
    private readonly apiKey: string,
    private readonly verifyTls: boolean,
    private readonly timeoutMs = 20_000,
  ) {}

  private sign(): { request_time: string; request_token: string } {
    const t = Math.floor(Date.now() / 1000).toString();
    const md5 = (s: string) => createHash('md5').update(s).digest('hex');
    return { request_time: t, request_token: md5(t + md5(this.apiKey)) };
  }

  /** Llama a una función de un plugin: POST /plugin?action=a&name=<plugin>&s=<fn> */
  plugin<T = unknown>(name: string, fn: string, params: Record<string, string | number> = {}): Promise<T> {
    const qs = new URLSearchParams({ action: 'a', name, s: fn });
    return this.post<T>(`/plugin?${qs}`, params);
  }

  post<T = unknown>(path: string, params: Record<string, string | number>): Promise<T> {
    const body = new URLSearchParams({
      ...this.sign(),
      ...Object.fromEntries(Object.entries(params).map(([k, v]) => [k, String(v)])),
    }).toString();
    const url = new URL(path, this.baseUrl);
    const mod = url.protocol === 'https:' ? https : http;

    return new Promise<T>((resolve, reject) => {
      const req = mod.request(
        url,
        {
          method: 'POST',
          headers: {
            'Content-Type': 'application/x-www-form-urlencoded',
            'Content-Length': Buffer.byteLength(body),
          },
          timeout: this.timeoutMs,
          ...(url.protocol === 'https:' ? { rejectUnauthorized: this.verifyTls } : {}),
        },
        (res) => {
          const chunks: Buffer[] = [];
          res.on('data', (c: Buffer) => chunks.push(c));
          res.on('end', () => {
            const text = Buffer.concat(chunks).toString('utf8');
            let data: unknown;
            try {
              data = JSON.parse(text);
            } catch {
              return reject(new AaPanelError(`Respuesta no válida del panel (HTTP ${res.statusCode})`));
            }
            const d = data as { status?: boolean; msg?: string };
            if (d && typeof d === 'object' && d.status === false) {
              return reject(new AaPanelError(d.msg || 'El panel rechazó la operación'));
            }
            resolve(data as T);
          });
        },
      );
      req.on('timeout', () => req.destroy(new AaPanelError('El panel no respondió a tiempo')));
      req.on('error', (e: NodeJS.ErrnoException) => {
        if (e instanceof AaPanelError) return reject(e);
        if (e.code === 'ECONNREFUSED') return reject(new AaPanelError('No se pudo conectar al panel (revisa AAPANEL_URL y el puerto)'));
        if (e.code === 'ENOTFOUND') return reject(new AaPanelError('No se encontró el host del panel (AAPANEL_URL)'));
        if (/certificate|self.signed/i.test(e.message)) {
          return reject(new AaPanelError('Certificado TLS del panel no válido (usa AAPANEL_VERIFY_TLS=false si es autofirmado)'));
        }
        reject(new AaPanelError(`Error al contactar el panel: ${e.code ?? e.message}`));
      });
      req.end(body);
    });
  }
}

export class AaPanelError extends Error {}

/** Extrae la lista de una respuesta de aaPanel, que según la versión llega en distintas formas. */
export function listFrom<T>(data: unknown): T[] {
  if (Array.isArray(data)) return data as T[];
  const d = data as { data?: unknown; msg?: unknown };
  if (Array.isArray(d?.data)) return d.data as T[];
  if (Array.isArray((d?.data as { data?: unknown })?.data)) return (d.data as { data: T[] }).data;
  if (Array.isArray(d?.msg)) return d.msg as T[];
  return [];
}
