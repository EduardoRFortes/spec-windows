# spec-windows

Monitor de permissões para o Claude Code no Windows: quando o Claude Code
for pedir permissão para rodar um comando ou editar um arquivo, em vez do
prompt normal do terminal, um ícone na bandeja do sistema (e uma toast
notification) deixa você aprovar ou negar com um clique — sem sair do que
estava fazendo.

Porta do [spec-fedora](https://github.com/EduardoRFortes/spec-fedora) (Fedora/
GNOME) para Windows. Mesmo protocolo, mesmo mecanismo (hooks oficiais do
Claude Code, sem scraping de terminal) — só a "cara" muda: AppIndicator3 +
libnotify → bandeja nativa do Windows (`Shell_NotifyIcon`) + toast.

Ver [PROTOCOL.md](./PROTOCOL.md) para o protocolo do pipe e os timeouts.

## Como funciona

```
Claude Code ── hook PreToolUse ──▶ spec-hook (Rust) ── named pipe ──▶ specd (Rust)
     ◀── allow / deny ◀──────────────────────────────────────────────◀── clique na bandeja
```

**Regra de ouro: fail-open.** Igual ao spec-fedora — se o daemon não
estiver rodando, travar, ou demorar demais, o hook sai silenciosamente e o
prompt normal do terminal aparece.

## Diferenças em relação ao spec-fedora

- **Transporte:** named pipe (`\\.\pipe\spec`) em vez de unix socket, via
  crate [`interprocess`](https://docs.rs/interprocess) — mesma API de
  stream dos dois lados, então o `hook/` fica quase idêntico ao original.
- **Daemon:** Rust (`tray-icon` + notificações nativas) em vez de
  Python/GTK3 — não faz sentido puxar GTK pro Windows, e dá pra escrever um
  daemon único binário sem runtime.
- **Sem systemd:** `specd` inicia via Tarefa Agendada do Windows
  (trigger "at logon", com reinício automático se cair) em vez de um
  serviço `systemd --user`.

## Status

Funcionando de ponta a ponta com o Claude Code real: hook de permissão,
notificação toast, barras de uso na própria bandeja e início automático no
boot.

## Instalação

```
.\install\install.ps1
```

Compila os binários em release, registra o hook `PreToolUse` e a
`statusLine` em `~/.claude/settings.json`, e cria/atualiza a Tarefa
Agendada `SpecWindowsTray` que sobe o `specd` a cada logon. Idempotente —
pode rodar de novo a qualquer momento (ex.: depois de um `git pull`) para
atualizar os binários.

## Componentes

- `hook/` — `spec-hook` (Rust), registrado como hook `PreToolUse`; e
  `spec-statusline`, registrado como `statusLine`.
- `daemon/` — `specd` (Rust), bandeja + notificações + estado dos pedidos
  pendentes + Tarefa Agendada para autostart.
- `install/` — `install.ps1` (fluxo completo) e `merge_settings.ps1`
  (registro do hook/statusLine em `~/.claude/settings.json`).
