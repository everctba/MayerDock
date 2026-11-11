# MayerDock

Dock lateral do Windows que **reserva uma faixa da tela** (protocolo AppBar, o mesmo da
barra de tarefas) e **encaixa janelas** de outras aplicações nessa faixa — até **3
empilhadas verticalmente** — mantendo um painel fixo sempre visível ao lado do trabalho.

Status: **0.1.0 — MVP funcional**. Detalhes de produto em [docs/PRD.md](docs/PRD.md).

> **Linux/Wayland:** existe um projeto separado em [MayerDock-Linux](MayerDock-Linux/README.md)
> — decisão de portabilidade em
> [ADR-001](MayerDock-Linux/docs/ADR-001-linux.md) e escopo em
> [PRD](MayerDock-Linux/docs/PRD.md). Por ora é proposta (não iniciado); foco **Wayland**. 

## O que ele faz

- Registra um AppBar na **borda direita** do monitor, encostado à esquerda da barra de
  tarefas (funciona com taskbar vertical na direita).
- As janelas maximizadas dos outros apps passam a terminar na borda do dock; a área de
  trabalho do Windows encolhe de verdade.
- O dock fica **sempre no topo** e fora do Alt+Tab — não dá para cobri-lo arrastando uma
  janela comum.
- **Arraste a borda esquerda** para redimensionar (a área de trabalho acompanha).
- **Divisões verticais (1, 2 ou 3):** no menu ☰, em **Divisões**, escolha quantas janelas
  cabem empilhadas; a divisão atual fica marcada com **•**.
- Clique no corpo do dock / botão **☰** e escolha uma janela aberta: ela ocupa a primeira
  divisão livre e é mantida ali (mover, redimensionar, re-encaixar).
- Botão **←** no cabeçalho (à esquerda do ☰): **libera todas as janelas encaixadas**,
  devolvendo cada uma à posição e ao tamanho originais; fica esmaecido quando o dock está
  vazio.
- **Liberar** no menu: libera uma divisão específica (mostra qual app está nela).
- **Ícone na bandeja:** clique duplo mostra/oculta o dock; botão direito abre o menu
  (mostrar/ocultar, sair). Ao ocultar manualmente (bandeja, `Ctrl+Alt+M` ou menu), as
  janelas encaixadas são **liberadas** (voltam ao lugar) — nenhum app fica escondido.
  O ícone é o **próprio do MayerDock**, embutido no `.exe` (`assets/icon.ico`) — o mesmo
  que aparece no Explorer, no Gerenciador de Tarefas e no atalho.
- O dock **some sozinho** quando um jogo ou vídeo entra em fullscreen (D3D exclusivo ou
  janela cobrindo o monitor) e volta quando o fullscreen termina.
- Se o **Explorer reiniciar**, o app re-registra automaticamente a **bandeja** e a **faixa
  do AppBar** (`TaskbarCreated`), sem precisar reiniciar o MayerDock.
- **Start, pesquisa, Task View e flyouts do shell:** enquanto um desses painéis está em
  primeiro plano o dock sai da frente (a área reservada é liberada), então a barra de
  pesquisa e o Start abrem no tamanho cheio em vez de ficarem cortados pelo dock; ao
  fechar, o dock volta.
- Largura e número de divisões persistidos em `%APPDATA%\MayerDock\config.ini`.
- **FluxTV (camada de escurecimento):** o MayerDock reergue as janelas do FluxTV acima
  de si (classes `FluxTV.*`), então o multiply/escurecimento também cobre a faixa do dock,
  as janelas encaixadas e o menu — os dois apps são topmost e sem isso o dock ficava por
  cima da camada.
- **Portátil ou instalador:** a pasta `dist\MayerDock\` roda direto (sem admin); ou use o
  `MayerDock-Setup.exe` para instalar com atalhos, desinstalador e escolha de pasta.

## Build

```powershell
.\build.ps1              # compila e monta dist\MayerDock\
.\build.ps1 -Run         # + abre o .exe da pasta portátil
.\build.ps1 -Shortcut    # + atalho "MayerDock.lnk" na área de trabalho
.\build.ps1 -Installer   # + instalador Windows em dist\installer\
.\build.ps1 -Msix        # + pacote MSIX em dist\msix\ (não substitui o instalador)
```

O build é **offline** e usa apenas `windows-sys` do cache local do Cargo. O `build.ps1`
escolhe o toolchain sozinho: MSVC quando existe `link.exe`; senão `rustup` +
`x86_64-pc-windows-gnu` com o primeiro gcc x86_64 encontrado (w64devkit/MSYS2/MinGW-w64),
criando o stub de `libgcc_eh.a` quando a distribuição não o traz.

Saídas:

```text
target\release\mayerdock.exe               # binário cru (não portátil: fica no build)
dist\MayerDock\                             # PASTA PORTÁTIL — copie/mova esta
├─ mayerdock.exe
├─ README.md
├─ LICENSE
└─ docs\PRD.md
dist\installer\MayerDock-Setup-x.y.z.exe    # INSTALADOR Windows (Inno Setup)
```

Rodando a versão portátil:

```powershell
.\dist\MayerDock\mayerdock.exe
```

> O `config.ini` e o `mayerdock.log` continuam em `%APPDATA%\MayerDock\` (por usuário),
> então mover a pasta não perde configuração. Se o MayerDock estiver aberto, feche antes
> de recompilar (o `.exe` fica travado).

## Linha de comando (diagnóstico)

```powershell
.\dist\MayerDock\mayerdock.exe --status    # versão, modo, config e caminhos reais
.\dist\MayerDock\mayerdock.exe --version
.\dist\MayerDock\mayerdock.exe --help
```

O `--status` mostra:

```text
version: 0.1.0
packaged: false                 # true quando rodando como MSIX
width_logical: 772
slots: 1
data_dir: C:\Users\...\AppData\Roaming\MayerDock
config: C:\Users\...\AppData\Roaming\MayerDock\config.ini
log: C:\Users\...\AppData\Roaming\MayerDock\mayerdock.log
```

No modo **MSIX** (`packaged: true`, com `package:` também), esses caminhos são os da
**área do pacote** (redirecionada pelo Windows) — é o endereço real para achar config/log.

## Instalador

Gerado com **Inno Setup 6** pela flag `-Installer`. O `build.ps1` procura o `ISCC.exe` em
`%LOCALAPPDATA%\Programs\Inno Setup 6`, em `Program Files` ou na variável
`INNO_SETUP_ISCC`; se não achar, avisa e segue só com a pasta portátil.

```powershell
.\build.ps1 -Installer
# -> dist\installer\MayerDock-Setup-0.1.0.exe
```

Comportamento do instalador:

- **página de escolha da pasta** (`DisableDirPage=no`) — você define onde instalar;
  padrão `%LOCALAPPDATA%\Programs\MayerDock` (por usuário, **sem admin**);
- oferece **"só para mim"** ou **"para todos os usuários"** (a segunda usa
  `Program Files` e pede UAC);
- **fecha o MayerDock em execução** antes de copiar (`taskkill` → WM_CLOSE, saída limpa,
  liberando as janelas e o AppBar);
- cria atalho no **Menu Iniciar** e, se marcado, na **área de trabalho**;
- aparece em **Aplicativos instalados** com desinstalador padrão e ícone próprio;
- opção **abrir o MayerDock** ao terminar;
- desinstalar remove programa + atalhos + entrada de desinstalação; o
  `%APPDATA%\MayerDock\config.ini` é **preservado**.

Instalar o Inno Setup sem admin (como foi feito nesta máquina):

```powershell
# baixe em https://jrsoftware.org/isdl.php e rode:
innosetup-6.x.x.exe /VERYSILENT /CURRENTUSER /SUPPRESSMSGBOXES /NORESTART
```

## MSIX (em paralelo ao instalador)

O mesmo build gera um pacote **MSIX** (F1/F2) sem tocar no Inno/portátil:

```powershell
.\build.ps1 -Msix                            # dist\msix\MayerDock-0.1.0.msix (sem assinatura)
.\installer\msix\build-msix.ps1 -SelfSign    # pacote assinado com certificado de dev
.\installer\msix\build-msix.ps1 -SelfSign -Install   # assina e instala (sideload)
.\installer\msix\build-msix.ps1 -Uninstall           # remove o pacote instalado
```

- **Tooling sem admin:** o script procura `makeappx.exe`/`signtool.exe` no Windows SDK, no
  PATH ou em `.tools\win-sdk-buildtools*\` (pacote NuGet
  `Microsoft.Windows.SDK.BuildTools`, versão **10.0.22621.x**; a 28000.x não roda no
  Windows 10 19041). `.tools\` é ignorado pelo git.
- **Sem escolha de pasta:** MSIX instala em `WindowsApps` — a escolha de pasta continua
  sendo exclusiva do instalador Inno.
- **Sideload** (validado em 2026-11-11): Modo de Desenvolvedor + certificado de dev em
  `LocalMachine\TrustedPeople` (uma vez, como admin) — sem isso o Windows recusa com
  `0x800B0109`. Pacote instalado ativa normalmente; AppBar, janelas e bandeja funcionam.
- **Dados:** em full trust o `%APPDATA%` **não** é redirecionado — `config.ini`/log ficam no
  caminho real e **sobrevivem** ao uninstall (mesmo comportamento do Inno).
- **Store:** suba o pacote **sem assinatura** — a Store assina. `Identity`/`Publisher` são
  parametrizáveis (`-IdentityName`, `-Publisher`).
- Não rode a versão Inno e a MSIX ao mesmo tempo (duas AppBars/trays). O `.exe` instalado
  em `WindowsApps` não roda direto — abra pelo Menu Iniciar/shell (`AppsFolder`).

## Como usar

1. Abra o MayerDock — o dock aparece na direita da tela (à esquerda da taskbar).
2. Em **☰ → Divisões**, escolha 1, 2 ou 3 janelas empilhadas.
3. Clique no corpo dele e escolha a primeira janela; repita para as demais divisões.
4. Trabalhe; maximize o que quiser — as janelas param na borda do dock.
5. **←** libera todas as janelas; **Sair do MayerDock** remove o dock e restaura a área de
   trabalho.

O recurso global: `Ctrl+Alt+M` alterna mostrar/ocultar o dock. Na bandeja, clique duplo
também alterna e o botão direito abre o menu.

## DPI e escala do Windows

O MayerDock é **per-monitor DPI aware v2** — o `.exe` embute um application manifest com
`PerMonitorV2` e `asInvoker`, e em runtime também chama `SetProcessDpiAwarenessContext`
(no-op quando o manifesto já definiu). Com *Display settings → Scale* acima de 100%:

- a interface (fontes, cabeçalho, botões, menu) **escala junto** — as métricas são
  definidas em pixels lógicos (96 dpi) e multiplicadas pelo fator do monitor (`dpi / 96`);
- a **largura** do dock é guardada em pixels lógicos (`width_logical` no `config.ini`) e a
  faixa do AppBar é reservada em pixels físicos convertidos;
- **mudança de escala em runtime** (mover para outro monitor com DPI diferente, alterar o
  Scale) dispara `WM_DPICHANGED`: fontes recriadas, faixa e divisões recalculadas;
- se um app encaixado tiver tamanho **mínimo** maior que a divisão (comum em escala alta),
  o dock cresce sozinho para caber, até o limite de 1200 px lógicos;
- configs antigas (chave `width`, em pixels físicos) são **migradas** automaticamente para
  `width_logical` no primeiro start, usando o DPI atual.

Para testar outra escala sem mexer no Windows:

```powershell
$env:MAYERDOCK_DPI = 144      # simula 150% (96 = 100%, 120 = 125%, 192 = 200%)
.\dist\MayerDock\mayerdock.exe
```

## Limitações conhecidas (MVP)

- Um dock por sessão, no monitor primário, sempre à direita.
- Até **3** janelas empilhadas (perfis salvos por app são P2 do PRD).
- O menu (☰) é uma **janela própria do MayerDock**, sempre no topo: ele aparece acima da
  janela encaixada **sem esconder nada**, e fecha com clique fora, `Esc` ou escolha.
  Navegação por teclado: `↑` `↓` + `Enter`; `Esc` cancela.
- Jogos em fullscreen exclusivo: o dock esconde, mas o app encaixado continua no lugar.
- Apps "sempre no topo" de terceiros podem sobrepor o dock.
- As janelas encaixadas são promovidas a *topmost* enquanto estão no dock; ao liberar (ou
  sair) voltamos cada uma ao normal e ao lugar original.
- Se o MayerDock for **morto à força** (Gerenciador de Tarefas / `Stop-Process`), as
  janelas encaixadas podem continuar *topmost* até serem restauradas à mão — sai sempre
  pelo ✕/menu/bandeja.

## Ícone

O ícone do app vive em `assets\`:

- `assets\icon.png` — arte de origem (512×512 RGBA), é o que se edita;
- `assets\icon.ico` — multi-tamanho (16/24/32/48/64/128/256) embutido no `.exe` pelo
  `build.rs` (via `windres` no GNU/MinGW ou `rc.exe` no MSVC) e usado na bandeja.

Para trocar o ícone: substitua `assets\icon.png` e regenere o `.ico` (Pillow), depois
recompile:

```powershell
python -c "from PIL import Image; Image.open(r'assets\icon.png').convert('RGBA').save(r'assets\icon.ico', sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])"
.\build.ps1
```

## Estrutura

```text
MayerDock\
├─ Cargo.toml
├─ build.rs         # embute ícone + versão no .exe (windres/rc.exe)
├─ build.ps1        # MSVC ou GNU + MinGW, build offline + pasta portátil
├─ assets\
│  ├─ icon.png      # arte de origem do ícone
│  ├─ icon.ico      # multi-tamanho, embutido no .exe
│  └─ app.manifest  # DPI PerMonitorV2 + asInvoker (embutido pelo build.rs)
├─ installer\
│  ├─ MayerDock.iss # script do instalador (Inno Setup 6)
│  └─ msix\         # manifesto (template), assets e build-msix.ps1
├─ MayerDock-Linux\ # projeto irmão (Wayland): ADR + PRD, sem código ainda
├─ docs\PRD.md      # PRD do produto
├─ docs\TASKS.md    # backlog/tarefas (ex.: build MSIX para a Store)
├─ docs\MSIX.md     # levantamento de requisitos MSIX/Store
└─ src\
   ├─ main.rs       # entrada
   ├─ win.rs        # utilitários Win32, monitor e log
   ├─ config.rs     # config.ini (largura + divisões)
   ├─ menu.rs       # menu popup próprio (janela topmost)
   └─ dock.rs       # janela, AppBar, divisões, bandeja, fullscreen
```
