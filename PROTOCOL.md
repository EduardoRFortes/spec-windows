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

## Endpoint HTTP (sessões remotas)

Além do named pipe, o `specd` também escuta `POST /usage` em
`127.0.0.1:27182` (porta configurável via `SPEC_HTTP_PORT`) — mesmo corpo
JSON da mensagem `usage` do pipe, fire-and-forget, sem resposta além de um
`200 OK` vazio. `read_http_body` em `daemon/src/main.rs` não é um parser
HTTP de verdade: só lê `Content-Length` dos headers e o body exato, o
suficiente para o único caso de uso que existe.

Existe porque o Claude Code rodando dentro de uma VM/container acessado via
SSH (incluindo VS Code Remote - SSH) nunca roda um binário Windows, então
nunca fala com o pipe. Nesse caso o `statusLine` da sessão remota é
`hook/spec-statusline-remote.py` (Python 3, stdlib only) em vez de
`spec-statusline.exe`, e ele faz o `POST` para `localhost:27182` — que só
chega no `specd` do Windows por causa de um túnel SSH reverso
(`RemoteForward 27182 localhost:27182`) configurado do lado Windows. Sem o
túnel ativo, o `POST` simplesmente falha e é ignorado — mesma regra
fail-open do resto do projeto, só que aqui do lado do `statusLine`, que já
não tem decisão nenhuma para travar.

Bind é só em `127.0.0.1`: o que torna a porta alcançável da VM é o túnel,
não a porta estar aberta pra fora.

## Timeouts (mesmos valores do spec-fedora)

- **Handshake (~300ms, no hook):** cobre "o daemon não está rodando/travado".
- **Decisão (~55s, no hook; 600s no daemon como rede de segurança):** cobre
  "o usuário ainda não clicou".

Em ambos os casos, "desistir" = `exit(0)` sem nada no stdout — fail-open.
