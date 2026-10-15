import { BadRequestException } from '@nestjs/common';
import { z, ZodTypeAny } from 'zod';

/** Valida un body/query con zod y devuelve errores legibles en español. */
export function parse<S extends ZodTypeAny>(schema: S, input: unknown): z.output<S> {
  const r = schema.safeParse(input);
  if (!r.success) {
    const detail = r.error.issues.map((i) => `${i.path.join('.') || 'valor'}: ${i.message}`).join('; ');
    throw new BadRequestException(`Datos inválidos — ${detail}`);
  }
  return r.data;
}
