# MayerDock

Dock lateral do Windows que **reserva uma faixa da tela** (protocolo AppBar, o mesmo da
barra de tarefas) e **encaixa janelas** de outras aplicações nessa faixa — até **3
empilhadas verticalmente** — o "Nível A" do conceito inspirado no *Dock to Desktop* do
OneNote.

Status: **0.1.0 — MVP funcional**. Detalhes de produto em [docs/PRD.md](docs/PRD.md).

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
  (mostrar/ocultar, sair).
- O dock **some sozinho** quando um jogo ou vídeo entra em fullscreen (D3D exclusivo ou
  janela cobrindo o monitor) e volta quando o fullscreen termina.
- **Start, pesquisa, Task View e flyouts do shell:** enquanto um desses painéis está em
  primeiro plano o dock sai da frente (a área reservada é liberada), então a barra de
  pesquisa e o Start abrem no tamanho cheio em vez de ficarem cortados pelo dock; ao
  fechar, o dock volta.
- Largura e número de divisões persistidos em `%APPDATA%\MayerDock\config.ini`.
- Sem admin, sem instalador: um `.exe` + `config.ini`.

## Build

```powershell
.\build.ps1            # compila (MSVC se houver link.exe; senão GNU + MinGW)
.\build.ps1 -Run       # compila e abre
```

O binário sai em `target\release\mayerdock.exe`. O build é **offline** e usa apenas
`windows-sys` do cache local do Cargo.

Rodando direto:

```powershell
.\target\release\mayerdock.exe
```

## Como usar

1. Abra o MayerDock — o dock aparece na direita da tela (à esquerda da taskbar).
2. Em **☰ → Divisões**, escolha 1, 2 ou 3 janelas empilhadas.
3. Clique no corpo dele e escolha a primeira janela; repita para as demais divisões.
4. Trabalhe; maximize o que quiser — as janelas param na borda do dock.
5. **←** libera todas as janelas; **Sair do MayerDock** remove o dock e restaura a área de
   trabalho.

O recurso global: `Ctrl+Alt+M` alterna mostrar/ocultar o dock. Na bandeja, clique duplo
também alterna e o botão direito abre o menu.

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
- O ícone da bandeja usa o ícone padrão do Windows por enquanto (ícone próprio é P1).

## Estrutura

```text
MayerDock\
├─ Cargo.toml
├─ build.ps1        # MSVC ou GNU + MinGW, build offline
├─ docs\PRD.md      # PRD do produto
└─ src\
   ├─ main.rs       # entrada
   ├─ win.rs        # utilitários Win32, monitor e log
   ├─ config.rs     # config.ini (largura + divisões)
   ├─ menu.rs       # menu popup próprio (janela topmost)
   └─ dock.rs       # janela, AppBar, divisões, bandeja, fullscreen
```
