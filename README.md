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
- **Sem systemd:** inicialização via Task Scheduler ou atalho na pasta
  Startup (a definir em `install/`).

## Status

Prototipagem inicial. Ainda não compilado/testado — falta instalar o
toolchain Rust (`rustup` + MSVC Build Tools) nesta máquina.

## Componentes (planejado, espelhando o spec-fedora)

- `hook/` — `spec-hook` (Rust), registrado como hook `PreToolUse`.
- `daemon/` — `specd` (Rust), bandeja + notificações + estado dos pedidos
  pendentes.
- `install/` — script de instalação (registro do hook em
  `~/.claude/settings.json`, atalho de inicialização).
