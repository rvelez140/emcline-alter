import { Controller, Inject, MessageEvent, OnModuleDestroy, Sse, UseGuards } from '@nestjs/common';
import { RvEvent } from '@rvmail/shared';
import Redis from 'ioredis';
import { filter, interval, map, merge, Observable, Subject } from 'rxjs';
import { AuthGuard, CurrentUser, SessionUser } from '../auth/auth.guard';
import { REDIS } from '../infra/infra.module';

type Envelope = { userId: string; event: RvEvent };

/**
 * Tiempo real: una sola suscripción Redis por proceso (patrón
 * rvmail:events:*) que se reparte en memoria a cada conexión SSE.
 */
@Controller('events')
export class EventsController implements OnModuleDestroy {
  private readonly sub: Redis;
  private readonly bus = new Subject<Envelope>();

  constructor(@Inject(REDIS) redis: Redis) {
    this.sub = redis.duplicate();
    void this.sub.psubscribe('rvmail:events:*');
    this.sub.on('pmessage', (_pattern, channel, payload) => {
      const userId = channel.slice('rvmail:events:'.length);
      try {
        this.bus.next({ userId, event: JSON.parse(payload) as RvEvent });
      } catch {
        /* evento mal formado: se ignora */
      }
    });
  }

  @Sse()
  @UseGuards(AuthGuard)
  stream(@CurrentUser() user: SessionUser): Observable<MessageEvent> {
    const events = this.bus.pipe(
      filter((e) => e.userId === user.id),
      map((e): MessageEvent => ({ type: e.event.type, data: e.event })),
    );
    // Ping cada 25 s para que proxies no cierren la conexión
    const ping = interval(25_000).pipe(map((): MessageEvent => ({ type: 'ping', data: String(Date.now()) })));
    return merge(events, ping);
  }

  onModuleDestroy() {
    this.sub.disconnect();
    this.bus.complete();
  }
}
