import {
  Body,
  Controller,
  Delete,
  Get,
  HttpCode,
  Param,
  ParseUUIDPipe,
  Patch,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { z } from 'zod';
import { AuthGuard, CurrentUser, hasScope, RequireScopes, SessionUser } from '../auth/auth.guard';
import { parse } from '../common/zod';
import { composeSchema, listSchema, MailService, sendSchema } from './mail.service';

const patchSchema = z.object({
  seen: z.boolean().optional(),
  flagged: z.boolean().optional(),
});

/**
 * API REST de correo. Acepta la sesión del navegador y también tokens de API
 * (Authorization: Bearer rvm_…) con los ámbitos indicados en cada endpoint.
 */
@Controller()
@UseGuards(AuthGuard)
export class MailController {
  constructor(private readonly mail: MailService) {}

  @Get('folders')
  @RequireScopes('mail:read')
  folders(@CurrentUser() user: SessionUser, @Query('accountId') accountId?: string) {
    return this.mail.listFolders(user, accountId);
  }

  @Get('messages')
  @RequireScopes('mail:read')
  list(@CurrentUser() user: SessionUser, @Query() query: unknown) {
    return this.mail.listMessages(user, parse(listSchema, query));
  }

  @Get('messages/:id')
  @RequireScopes('mail:read')
  get(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string) {
    // Un agente con solo lectura no altera el estado "leído" del buzón
    return this.mail.getMessage(user, id, { markSeen: hasScope(user, 'mail:modify') });
  }

  @Patch('messages/:id')
  @RequireScopes('mail:modify')
  patch(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string, @Body() body: unknown) {
    return this.mail.setFlags(user, id, parse(patchSchema, body));
  }

  /** Mueve a la papelera (o borra definitivamente si ya está en la papelera). */
  @Delete('messages/:id')
  @HttpCode(204)
  @RequireScopes('mail:modify')
  async remove(@CurrentUser() user: SessionUser, @Param('id', ParseUUIDPipe) id: string) {
    await this.mail.remove(user, id);
  }

  @Post('send')
  @HttpCode(200)
  @RequireScopes('mail:send')
  send(@CurrentUser() user: SessionUser, @Body() body: unknown) {
    return this.mail.send(user, parse(sendSchema, body));
  }

  @Post('drafts')
  @HttpCode(201)
  @RequireScopes('mail:draft')
  draft(@CurrentUser() user: SessionUser, @Body() body: unknown) {
    return this.mail.saveDraft(user, parse(composeSchema, body));
  }
}
