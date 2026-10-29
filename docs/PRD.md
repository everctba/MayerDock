# PRD — MayerDock

**Versão:** 0.1 (rascunho para revisão) · **Data:** 2026-10-29 · **Status:** MVP em construção

---

## 1. Resumo

MayerDock é um utilitário Windows (Rust, Win32 puro) que cria um **dock lateral que reserva
uma faixa da tela** — o mesmo mecanismo que a barra de tarefas usa (protocolo **AppBar**).
A área útil do Windows encolhe de verdade: janelas maximizadas param na borda do dock, e
não é possível cobri-lo arrastando outra janela por cima. Dentro da faixa, o usuário
**encaixa a janela de qualquer aplicativo** (navegador, editor, player) — o "Nível A" do
conceito inspirado no *Dock to Desktop* do OneNote.

## 2. Contexto e problema

O comportamento de referência é o **Dock to Desktop** do OneNote:

- o painel fica preso a uma borda (normalmente a direita), ocupa a altura toda e fica
  sempre visível;
- o restante do desktop continua utilizável e as janelas maximizadas **param na borda**
  do painel (a área de trabalho foi reduzida);
- uma janela comum arrastada para cima do painel fica **atrás** dele.

O que falta no Windows:

- não existe recurso nativo de "split reservado" genérico (o Snap/FancyZones organiza
  janelas, mas não reserva a faixa: qualquer app maximizado cobre a área);
- não existe forma nativa de colocar um app qualquer dentro de uma janela de outro app;
- o usuário também tem um caso concreto de tela com metade do backlight queimada (TV usada
  como monitor) e precisa que o desktop "viva" só na metade boa.

**Decisão de produto:** replicar o comportamento do OneNote generalizado — dock
redimensionável, com **uma janela externa encaixada por vez** no MVP — e, no futuro, um
modo "reservar região morta" da TV.

## 3. Objetivos

| # | Objetivo |
| --- | --- |
| O1 | Reservar uma faixa da tela (AppBar) de verdade, com maximizadas respeitando o dock. |
| O2 | Permitir encaixar a janela de **qualquer** aplicativo na faixa (Nível A). |
| O3 | Não atrapalhar: sumir automaticamente em fullscreen (jogo/vídeo) e não cobrir o que o usuário está vendo. |
| O4 | Ser leve e sem admin: um `.exe` portátil, sem instalador. |
| O5 | Deixar a arquitetura pronta para abas/perfis e para o "modo metade da TV". |

### Não-objetivos (MVP)

- Embutir janelas de outros processos dentro da nossa (`SetParent`). Comprovadamente
  frágil (UWP, Chrome/Electron, DPI, foco) — **rejeitado** como base de produto.
- Gerenciador de janelas geral (substituir FancyZones, tiling completo).
- Multi-monitor configurável, múltiplos docks simultâneos, dock à esquerda no MVP (P1).
- Linux/macOS, instalador/MSI, auto-update.

## 4. Usuário e cenários

**Persona:** desenvolvedor/power user Windows que trabalha com a tela dividida e quer um
painel fixo (navegador de documentação, YouTube, chat, notas) sempre visível, sem
reorganizar janelas a cada maximizada.

**Cenários:**

1. Abrir o MayerDock → dock aparece à direita, encostado à esquerda da taskbar vertical.
2. Escolher o navegador → ele passa a ocupar a faixa; maximizar o VS Code no restante.
3. Arrastar a borda do dock para alargar → navegador e VS Code se ajustam.
4. Entrar em um jogo fullscreen → o dock (e o app encaixado) somem sozinhos; ao sair,
   voltam.
5. Clicar em **Liberar janela** → navegador volta para a posição/tamanho originais.
6. Fechar o MayerDock → a área de trabalho é restaurada (a faixa deixa de ser reservada).

## 5. Escopo — MVP (P0)

| ID | Requisito | Critério |
| --- | --- | --- |
| RF-01 | Registrar AppBar na borda direita, à esquerda da taskbar | Maximizar qualquer app deixa a faixa do dock livre |
| RF-02 | Janela do dock sempre no topo e fora do Alt+Tab | Arrastar janela sobre o dock não a cobre; Alt+Tab não lista o dock |
| RF-03 | Redimensionar arrastando a borda esquerda | Largura 240–1200 px; área útil acompanha em tempo real |
| RF-04 | Encaixar janelas em divisões verticais (1–3) | Menu ☰ → **Divisões** define 1, 2 ou 3; cada janela escolhida ocupa a primeira divisão livre e permanece lá (inclusive ao redimensionar o dock) |
| RF-05 | Liberar/restaurar janelas encaixadas | Botão **←** libera **todas**; o menu libera uma divisão específica (mostrando o app); cada janela volta ao tamanho e à posição originais, sem topmost |
| RF-06 | Menu de contexto (corpo, ☰ e botão direito) | Lista janelas abertas, divisões (1/2/3), liberar por divisão e sair |
| RF-07 | Ocultar em fullscreen | D3D exclusivo (`SHQueryUserNotificationState`) e janela cobrindo o monitor |
| RF-08 | Restaurar depois do fullscreen | Dock volta na posição e re-registra o AppBar |
| RF-09 | Persistir largura e divisões em `%APPDATA%\MayerDock\config.ini` | Reabrir mantém a largura e o número de divisões |
| RF-10 | Sair restaurando tudo | Área de trabalho volta ao normal; todas as janelas liberadas |
| RF-11 | Instância única + atalho global `Ctrl+Alt+M` | Segunda instância só traz o dock à frente; atalho alterna visibilidade |
| RF-12 | Tolerar reinício do Explorer / mudança de resolução | `ABN_POSCHANGED` e `WM_DISPLAYCHANGE` reaplicam o AppBar |
| RF-13 | Sair da frente para Start/pesquisa/Task View | O dock oculta e libera a área reservada (painel abre inteiro); ao fechar, o dock volta |
| RF-14 | Ícone na bandeja | Clique duplo mostra/oculta; botão direito abre menu (mostrar/ocultar, sair); sai sempre pelo ✕/menu/bandeja |

## 6. Fora do MVP (backlog)

- **P1:** ícone próprio na bandeja (hoje usa o ícone padrão do Windows), dock à esquerda,
  escolher monitor, atalhos para "encaixar janela em foco".
- **P2:** perfis salvos por app, divisões desiguais (redimensionar a divisória), modo
  "reservar metade morta da TV", auto-hide do dock, tema claro.
- **P3:** múltiplos docks, presets de layout, animação de entrada.

## 7. Requisitos não-funcionais

- **RNF-01 Desempenho:** CPU ~0% parado (timer de 1 s é o único polling); RAM < 20 MB.
- **RNF-02 Robustez:** se o app encaixado fechar, o dock volta ao estado vazio; se o
  Explorer reiniciar, o AppBar é re-registrado; sair normal restaura a área de trabalho.
- **RNF-03 Portabilidade:** `.exe` único, sem admin, sem instalador; config em `%APPDATA%`.
- **RNF-04 Compatibilidade:** Windows 10/11 x64; DPI per-monitor v2; taskbar vertical na
  direita (caso do usuário).
- **RNF-05 Build offline:** apenas `windows-sys` (crate já em cache); MSVC ou MinGW-w64.

## 8. Arquitetura

```text
main.rs ──► dock::run(Config)
                │
                ├── register_class / CreateWindowExW (WS_POPUP, TOOLWINDOW, TOPMOST)
                ├── register_appbar (ABM_NEW, uCallbackMessage = WM_APP+10)
                ├── apply_appbar  (ABM_QUERYPOS → força largura → ABM_SETPOS → SetWindowPos)
                ├── message loop  (GetMessage/DispatchMessage)
                ├── WM_TIMER 1 s  → update_visibility + sync_slot
                ├── WM_APP+10 (AppBar) → ABN_POSCHANGED / ABN_FULLSCREENAPP
                └── WM_DESTROY → libera slot, ABM_REMOVE, salva config, PostQuitMessage
```

**Encaixe (Nível A):** o dock **não** reparenta a janela. Ele guarda `HWND` + retângulo
original, move/redimensiona a janela para a área útil (`SetWindowPos`), promove a janela a
`HWND_TOPMOST` (para não ficar atrás do dock) e reafirma a posição no timer. Ao liberar,
restaura `HWND_NOTOPMOST` + posição original. Funciona com qualquer app.

**Fullscreen:** `SHQueryUserNotificationState == QUNS_RUNNING_D3D_FULL_SCREEN` cobre jogos
exclusivos; comparação do retângulo da janela em foco com o retângulo do monitor cobre
vídeo/jogo borderless. Ao ocultar: `ABM_REMOVE` (devolve a área de trabalho) + esconde a
janela encaixada.

**Estado:** `static mut` acessado somente pela thread da interface (single-thread Win32).

## 9. UX

```text
┌───────────────────────────┬──────────────────┐
│                           │  MayerDock   ☰ ✕ │  ← cabeçalho 40 px, topmost
│   janela maximizada       ├──────────────────┤
│   (área de trabalho       │                  │
│    reduzida pelo AppBar)  │  janela          │  ← área útil do slot (1 janela)
│                           │  encaixada       │
│                           │                  │
└───────────────────────────┴──────────────────┘
                         taskbar vertical (direita)
```

- Borda esquerda: 6 px de arrasto para redimensionar (cursor ↔).
- Clique no corpo vazio / ☰ / botão direito: abre o **menu do MayerDock** — uma janela
  topmost própria (não o menu nativo), então ele aparece acima da janela encaixada sem
  precisar escondê-la. Itens: "Escolher janela" (lista com `Processo — Título`),
  "Liberar janela atual" e "Sair do MayerDock". Teclado: `↑↓`, `Enter`, `Esc`.
- `Ctrl+Alt+M`: mostra/oculta o dock (e libera a faixa quando oculto).

## 10. Critérios de aceite (MVP)

1. Ao abrir, a área de trabalho útil encolhe exatamente da largura do dock; maximizar o
   Explorer/VS Code termina na borda do dock.
2. Arrastar uma janela comum para cima do dock: ela fica atrás (dock topmost).
3. Arrastar a borda esquerda do dock altera a largura e as maximizadas acompanham.
4. Escolher uma janela no menu a coloca na área útil; redimensionar o dock a mantém ajustada.
5. Fechar a janela encaixada → dock mostra o estado vazio (sem quebrar).
6. Entrar em vídeo fullscreen no navegador → dock e app encaixado somem; ao sair, voltam.
7. Sair do MayerDock → área de trabalho restaurada (janelas maximizadas voltam a ocupar a
   tela toda) e janela encaixada volta ao lugar original.
8. Reabrir o MayerDock mantém a largura anterior.
9. Segunda instância não duplica o dock; traz o existente à frente.
10. `Ctrl+Alt+M` alterna visível/oculto sem perder a configuração.

## 11. Riscos e mitigações

| Risco | Impacto | Mitigação |
| --- | --- | --- |
| AppBar é API legada (mas viva no Win11) | Médio | Fallback documentado; testar nas builds atuais |
| App "sempre no topo" de terceiros cobre o dock | Baixo | Reaplicar topmost no timer/ABN_* |
| Crash deixa janela encaixada topmost | Médio | Restaurar no `WM_DESTROY`; documentar |
| Fullscreen borderless detectado tarde | Baixo | Polling de 1 s + `ABN_FULLSCREENAPP` |
| Multi-monitor/DPI | Médio | MVP no monitor primário; P1 |
| Explorer reiniciado perde o AppBar | Médio | Re-registrar em `WM_SETTINGCHANGE`/`ABN_POSCHANGED` |

## 12. Roadmap

1. **Fase 1 (este MVP):** AppBar direita, slot único, fullscreen, config, atalho.
2. **Fase 2:** abas/perfis, dock à esquerda, escolher monitor, bandeja.
3. **Fase 3:** modo "metade da TV" + presets.

## 13. Decisões registradas

- **Nível A** (janela externa encaixada por posicionamento) em vez de embedding
  (`SetParent`) — compatibilidade total e sem bugs de foco/render.
- **Dock à direita**, porque a taskbar do usuário é vertical à direita: ordem final
  `taskbar > dock > desktop`.
- **Sair do dock em fullscreen** é requisito, não opção.
- Nome do projeto: **MayerDock**.
