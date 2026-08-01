# Contexto de sessão — spec-windows

Atualizado em 2026-07-30. Leia isto no início da próxima sessão em vez de
reconstruir o histórico.

**Pasta do projeto:** `C:\Users\rodri\Documents\spec-windows`
(repo git local, remoto: `https://github.com/EduardoRFortes/spec-windows`)

## Estado do git

- Branch `master`. Havia chegado ao dia em dia com `origin/master` (`75599c3`)
  no início desta sessão; **nesta sessão houve uma mudança de código não
  commitada ainda** (ver seção "ícones fantasma" abaixo) — checar
  `git status`/`git diff` antes de assumir que o working tree está limpo.
- Working tree tinha, além disso, `stash@{0}`: "debug eprintln instrumentation
  from tunnel investigation" — instrumentação de debug temporária de uma
  investigação de túnel SSH antiga, ainda não descartada nem reaplicada.
  Decidir se dá pra jogar fora (`git stash drop`); a causa raiz daquele
  problema já foi resolvida por outra via (ver histórico de commits).

## O que é o projeto

Porta para Windows do `spec-fedora`: um daemon de bandeja (`specd.exe`) que
intercepta o hook `PreToolUse` do Claude Code via named pipe, mostra
permissões pendentes (Allow/Deny) e barras de uso (5h/semana) na bandeja do
Windows. Componentes:

- `daemon/` — `specd.exe`, app de bandeja (winit + tray-icon 0.19 + named
  pipe via `interprocess`).
- `hook/` — `spec-hook.exe`, o binário chamado pelo Claude Code no
  `PreToolUse`, fala com o daemon pelo named pipe. Também
  `spec-statusline-remote.py`, usado por sessões remotas (VM via SSH).
- `protocol/` — tipos compartilhados do protocolo (pipe JSON por linha).
- `install/install.ps1` — build release + registra hook/statusLine no
  `settings.json` do Claude Code + registra a Scheduled Task
  `SpecWindowsTray` (inicia o `specd.exe` no logon).

## Sessão de 2026-07-30: túnel remoto (VM via VS Code Remote-SSH) não funcionava

Usuário reportou que, neste computador (diferente do do trabalho), a sessão
de Claude Code numa VM acessada via VS Code Remote-SSH + VPN não aparecia na
bandeja do Windows — suspeita inicial era a VPN. **Não era a VPN.** Dois bugs
reais, resolvidos e já commitados/pushados:

### 1. `~/.ssh/config` com porta/host errados (config desatualizada)

A entrada manual do `Host` para a VM tinha
`RemoteForward 27182 localhost:27182` — exatamente os dois problemas que o
README já documenta como resolvidos na seção "Sessões remotas":
- Porta `27182` é interceptada pelo VS Code Remote-SSH (redireciona pra uma
  porta interna própria, nunca chega no `specd`); tem que ser `27283`.
- `localhost` resolve pra IPv6 primeiro no cliente SSH do Windows, mas o
  `specd` só escuta em IPv4; tem que ser `127.0.0.1`.

**Correção:** removido o `RemoteForward` manual do `~/.ssh/config` (deixando
só `HostName`/`User`/`Port`/`IdentityFile` pro alias `minha-vm`, que o VS
Code Remote-SSH ainda usa pra conexão normal) e criada a Scheduled Task
**`SpecSSHTunnel`** (método recomendado do README — túnel dedicado,
reconecta sozinho, não depende da sessão do VS Code estar aberta). O comando
de setup foi rodado ad-hoc, não faz parte do `install.ps1`; se precisar
recriar, o script está documentado no README, seção "Sessões remotas" →
"Método recomendado".

### 2. Bug real: `spec-statusline-remote.py` nunca dava flush no stdout

Mesmo com o túnel correto, a bandeja só refletia a sessão local do Windows,
nunca a da VM. Diagnóstico (via log de debug temporário injetado no script
real na VM, depois removido): o `print(line)` do script Python não tinha
`sys.stdout.flush()`. Como o stdout é um pipe (não um tty) quando o Claude
Code chama o hook, o Python usa buffer completo por padrão — a linha só
sairia no fechamento natural do processo. O Claude Code aparentemente mata/
descarta o processo assim que já capturou a saída, então o
`forward_to_daemon()` (que rodava *depois* do `print`) nunca tinha chance de
terminar — o POST HTTP pro `specd` nunca acontecia de verdade, apesar de
tudo mais (túnel, `specd`, porta) estar 100% correto.

O binário Rust local (`spec-statusline.exe`) nunca teve esse problema porque
`println!` do Rust sempre faz flush por linha, mesmo sem terminal.

**Correção:** adicionado `sys.stdout.flush()` logo após o `print(line)` em
`hook/spec-statusline-remote.py`, com comentário explicando o porquê.
Commitado (`75599c3`) e com push feito pro `origin/master`. Reimplantado na
VM (`/home/fortes/spec-statusline-remote.py`) e confirmado funcionando —
usuário viu a bandeja refletir a sessão da VM em tempo real.

### Ferramentas de diagnóstico usadas (não deixar isso te confundir de novo)

- Testes manuais com `curl` direto no comando SSH deram **falso positivo**
  várias vezes: aspas duplas do JSON eram comidas na cadeia
  PowerShell → `ssh.exe` → bash antes de chegar no `curl`, produzindo um
  corpo JSON inválido — mas o `specd` sempre responde `200 OK` mesmo quando
  o parse do `usage` falha (é fire-and-forget, ver `daemon/src/main.rs`
  `handle_http_connection`), então o `200` não prova nada por si só. Pra
  testar POST manual via SSH em PowerShell sem cair nessa armadilha: grave o
  JSON num arquivo (via base64 do lado do PowerShell + `base64 -d` no
  remoto) e use `curl --data @arquivo`, nunca `-d` com JSON inline dentro de
  aspas aninhadas.
- Pra achar o bug de verdade, o método que funcionou foi trocar
  temporariamente o script real na VM por uma cópia que loga cada etapa
  (timestamp, payload, resultado do POST) em `/tmp/spec-debug2.log`, pedir
  pro usuário gerar uma interação real no terminal do VS Code, e ler o log.
  Rodar o script manualmente via SSH não reproduz o bug (só falha quando é
  o Claude Code de verdade quem invoca).

## Ícones "fantasma" do Spec na bandeja — teoria corrigida: Fast Startup, não idle no login

**Correção do usuário (sessão seguinte):** a teoria de "horas paradas na tela
de login" estava errada. O padrão real é mais simples e mais consistente:
**o bug acontece no primeiro boot depois de um "desligar" do Windows**
(qualquer um, não depende de quanto tempo fica parado na tela de login).
Um *restart* explícito sempre resolve (`specd` volta a mostrar 1 ícone só).
A evidência do log de eventos da sessão anterior (gap de 8h entre boot e
logon em 29/07) provavelmente foi coincidência — aquela ocorrência também
era só o primeiro boot de um desligar, o idle longo era ruído.

**Causa raiz provável: Fast Startup (hiberboot) está ativado nesta máquina**
(confirmado: `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power\HiberbootEnabled = 1`,
e `powercfg /a` lista "Inicialização Rápida" como estado de suspensão
disponível). Com Fast Startup ligado, um "desligar" normal do Windows não é
um desligamento de verdade: ele faz logoff completo das sessões de usuário
(então `specd.exe`, iniciado pela Scheduled Task `AtLogOn`, sempre começa do
zero normalmente), mas **hiberna a sessão de kernel** (drivers, serviços de
sistema) em `hiberfil.sys` em vez de reiniciá-la de verdade. Um *restart*
sempre força reinicialização completa do kernel, sem esse atalho — batendo
exatamente com "boot = bug, restart = ok". A hipótese é que a saída da
hibernação parcial do kernel cria alguma disputa/race na inicialização de
drivers/serviços do shell que o boot 100% frio (via restart) não tem,
afetando o registro do ícone de bandeja do `specd` bem no início do logon.

**Ação decidida:** desativar o Fast Startup (`HiberbootEnabled = 0`) como
teste direto da teoria. Requer admin (chave em `HKLM`); a sessão do Claude
não estava elevada, então o comando foi passado pro usuário rodar numa
janela de PowerShell como administrador:
```powershell
Set-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Power" -Name HiberbootEnabled -Value 0
```
(ou via Painel de Controle → Opções de Energia → "Escolher o que os botões
de energia fazem" → desmarcar "Ativar inicialização rápida"). **Ao
retomar: perguntar se o usuário já desativou e se os fantasmas pararam de
aparecer depois de um ciclo desligar→ligar (não restart) com Fast Startup
desligado.**

### Nova mitigação: guard de instância única (mutex nomeado)

Usuário perguntou se dava pra corrigir no `specd` em vez de depender de
manter Fast Startup desligado pra sempre. Investigando o código, achamos uma
lacuna real e independente da teoria do Fast Startup: **`specd` não tinha
nenhuma proteção contra duas instâncias rodando ao mesmo tempo** — nada no
pipe listener (`ListenerOptions`/named pipe do Windows aceita múltiplas
instâncias por padrão) nem no `main()` impedia isso. Se por qualquer motivo
(hiberboot, race no logoff, o que for) duas cópias do `specd.exe` chegassem
a coexistir mesmo que por um instante, cada uma registraria seu próprio
ícone de bandeja — explicando bem o padrão de *múltiplos* ícones fantasma
(não só 1) se isso se acumulasse ao longo de vários ciclos de boot.

**Correção aplicada** (`daemon/src/main.rs`, `daemon/Cargo.toml`):
`acquire_single_instance_lock()` cria um mutex nomeado do Windows
(`CreateMutexW`, nome `Local\SpecWindowsTrayDaemon`, via `windows-sys` —
features `Win32_System_Threading` e `Win32_Security` adicionadas). Se
`GetLastError() == ERROR_ALREADY_EXISTS`, uma outra instância já está
rodando e o processo novo sai imediatamente, antes de criar event loop ou
ícone. Chamado logo na primeira linha de `main()`. O handle nunca é fechado
de propósito — precisa ficar vivo até o processo terminar, e o Windows
libera sozinho no fim do processo (encerramento normal, `TerminateProcess`,
crash, tanto faz).

**Testado nesta sessão:** compilado (`cargo build --release --workspace`,
limpo), `specd` reiniciado via `Start-ScheduledTask SpecWindowsTray` (pid
8836), e confirmado na prática: rodar `specd.exe` manualmente uma segunda
vez enquanto o primeiro já estava de pé resultou no segundo processo saindo
sozinho (`ExitCode 0`) sem criar ícone — só o processo original continuou
rodando. **Ainda não commitado.**

Isso não prova que essa era a causa exata dos fantasmas anteriores (não dá
pra reproduzir o cenário real de boot sob demanda), mas é uma blindagem
correta e de baixo risco que deveria eliminar duplicidade de ícone por
completo, goal do usuário de eventualmente poder religar o Fast Startup com
mais confiança.

### Mitigação de código da sessão anterior (mantida, mas não é mais a hipótese principal)

`daemon/main.rs`/`Cargo.toml` ganharam nesta sessão anterior um polling a
cada 30s do PID dono da janela `Shell_TrayWnd` do Explorer (`explorer_pid()`,
via `windows-sys`/`FindWindowW`+`GetWindowThreadProcessId`); se o PID mudar
entre checagens, `specd` força reconstrução do próprio ícone em vez de só
confiar no broadcast `TaskbarCreated`. Continua sendo uma rede de segurança
razoável para qualquer restart do Explorer perdido pelo broadcast passivo,
mesmo que a causa raiz real do bug reportado seja o Fast Startup — as duas
coisas não são mutuamente exclusivas. Compilado e testado em runtime,
**ainda não commitado** (junto com essa atualização do contexto).

## Notas gerais / preferências observadas

- Usuário se comunica em português; manter respostas em PT-BR.
- Prefere que eu confirme antes de ações disruptivas visíveis, mas em modo
  auto costuma deixar seguir com ações reversíveis (rename de arquivo,
  edição de config, criação de scheduled task) sem perguntar cada passo.
- Ao investigar bugs "funciona lá, não funciona aqui", vale desconfiar tanto
  da causa óbvia (aqui: VPN) quanto da própria metodologia de teste (aqui: o
  `curl` mentindo por causa de quoting) antes de aceitar qualquer diagnóstico
  como definitivo — só o log real do processo real, disparado pelo caminho
  real, é confiável.
