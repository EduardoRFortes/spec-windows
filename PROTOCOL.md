# Protocolo do pipe

Named pipe do Windows, newline-delimited JSON, uma linha por mensagem.
Mesmo formato de mensagens do [spec-fedora](https://github.com/EduardoRFortes/spec-fedora/blob/master/PROTOCOL.md),
só o transporte muda (unix socket → named pipe).

Path padrão: `\\.\pipe\spec` (named pipes do Windows não moram no
filesystem, então não há diretório/permissões equivalentes ao `0700` do
Unix — o ACL padrão de named pipe já restringe a sessão do usuário atual).
Pode ser sobrescrito com a variável `SPEC_PIPE`, útil para testes manuais.

## Quando o hook nem conecta

Igual ao spec-fedora: `spec-hook` olha o campo `permission_mode` do payload
que recebeu do Claude Code. Se não for exatamente `"default"`, sai
imediatamente em fail-open sem nem tentar o pipe.

## Sequência

```
spec-hook                                   specd
    │  connect()                               │
    │ ─────────────────────────────────────▶   │
    │  {"type":"request", ...}\n                │
    │ ─────────────────────────────────────▶   │
    │                                           │  registra pending,
    │                                           │  atualiza bandeja/toast
    │  {"type":"ack","request_id":"..."}\n      │
    │ ◀─────────────────────────────────────    │
    │           (espera clique do usuário)      │
    │  {"type":"decision", "decision":"allow|deny", "reason":"..."}\n
    │ ◀─────────────────────────────────────    │
    │  connection closed                        │
```

## Mensagens

Idênticas ao spec-fedora — `request` (hook → daemon), `ack` (daemon →
hook), `decision` (daemon → hook), `usage` (hook → daemon, fire-and-forget
do `spec-statusline`). Ver o PROTOCOL.md original para os exemplos de JSON
e o detalhe de cada campo; não repetido aqui para não divergir por
descuido — se o formato mudar, muda nos dois repos.

**Uma diferença real, não só de transporte:** `request_id` aqui é o
`tool_use_id` do payload do hook (fallback: `prompt_id`, depois
`<session_id>-<tool_name>`) — não só `prompt_id` como o spec-fedora original
documenta. Testando com o Claude Code de verdade no Windows, um único
`prompt_id` é compartilhado por *todas* as chamadas de ferramenta dentro do
mesmo turno (um turno pode chamar várias ferramentas), então usá-lo sozinho
como chave faz um pedido pendente sobrescrever outro em silêncio quando mais
de uma chamada está em voo ao mesmo tempo. `tool_use_id` é único por
chamada.

## Timeouts (mesmos valores do spec-fedora)

- **Handshake (~300ms, no hook):** cobre "o daemon não está rodando/travado".
- **Decisão (~55s, no hook; 600s no daemon como rede de segurança):** cobre
  "o usuário ainda não clicou".

Em ambos os casos, "desistir" = `exit(0)` sem nada no stdout — fail-open.
