# Agentes de IA: leer y enviar correo con RV Mail (MCP)

RV Mail incluye un servidor **MCP** (Model Context Protocol) en `/api/mcp`. Con
él, Claude (Claude Code, Claude Desktop, la API de Anthropic) o cualquier
cliente compatible con MCP puede buscar, leer, redactar y enviar correo a través
de tus cuentas, incluidos los buzones de tu propio servidor.

## 1. Crear un token

RV Mail → **Configuración → Acceso para IA → Nuevo token**. Elige:

| Permiso | Herramientas MCP | Uso |
|---|---|---|
| `mail:read` | `list_accounts`, `list_folders`, `search_messages`, `read_message` | Consultar |
| `mail:draft` | `create_draft` | El agente deja la respuesta en Borradores y **tú la revisas y envías** (recomendado para empezar) |
| `mail:send` | `send_email` | Envía directamente |
| `mail:modify` | `mark_message`, `move_to_trash` | Organizar |

También puedes limitar el token a ciertas cuentas y ponerle caducidad. El token
(`rvm_…`) se muestra **una sola vez**; RV Mail solo guarda su hash. Revócalo
cuando quieras desde la misma pantalla.

## 2. Conectar el agente

Claude Code:

```bash
claude mcp add --transport http rvmail https://correo.tudominio.com/api/mcp \
  --header "Authorization: Bearer rvm_xxxxxxxx"
```

Otros clientes (JSON):

```json
{
  "mcpServers": {
    "rvmail": {
      "type": "http",
      "url": "https://correo.tudominio.com/api/mcp",
      "headers": { "Authorization": "Bearer rvm_xxxxxxxx" }
    }
  }
}
```

API de Anthropic (conector MCP): usa la misma URL y el token como
`authorization_token`.

## 3. API REST con el mismo token

```bash
curl -H "Authorization: Bearer rvm_…" "https://correo.tudominio.com/api/messages?q=factura&limit=10"
curl -H "Authorization: Bearer rvm_…" https://correo.tudominio.com/api/messages/<id>
curl -H "Authorization: Bearer rvm_…" -H 'Content-Type: application/json' \
  -d '{"accountId":"…","to":["cliente@ejemplo.com"],"subject":"Hola","html":"<p>…</p>"}' \
  https://correo.tudominio.com/api/drafts      # o /api/send
```

Los tokens solo sirven en los endpoints de correo; nunca para gestionar cuentas,
usuarios, tokens ni el servidor de correo.

## Seguridad

- **Límite de envíos** por token: `AGENT_SEND_LIMIT_PER_HOUR` (30 por defecto).
  Protege la reputación de tus dominios si un agente se equivoca.
- Un token de solo lectura **no** marca los correos como leídos.
- El contenido de los correos viene de terceros: el servidor MCP indica al agente
  que lo trate como datos, nunca como instrucciones (riesgo de *prompt
  injection*). Para agentes autónomos usa `mail:draft` en lugar de `mail:send`.
- Cada envío de un agente queda registrado en el log de la API con el nombre del token.
