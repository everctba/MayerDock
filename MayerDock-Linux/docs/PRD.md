# PRD — MayerDock-Linux (Wayland)

**Versão:** 0.1 (rascunho) · **Data:** 2026-11-06 · **Status:** proposta, não iniciado
**Relacionado:** [ADR-001-linux.md](ADR-001-linux.md) (decisão de arquitetura)

---

## 1. Resumo

Dock lateral para Linux **Wayland**: reserva uma faixa à direita da tela e mantém até
três janelas empilhadas verticalmente, como o MayerDock do Windows. Por limitação do
Wayland, o dock **reserva o espaço**; o **encaixe** das janelas é feito pelo compositor
(regras/script), não pelo app.

## 2. Problema

No Windows o MayerDock usa AppBar + manipulação de janelas alheias. No Wayland:

- não existe AppBar; reservar área é `zwlr_layer_shell_v1` + `set_exclusive_zone`;
- um cliente **não pode mover/redimensionar janelas de outros clientes** (por design);
- "sempre no topo", bandeja e detecção de fullscreen mudam de mecanismo;
- compositores diferem (wlroots x KWin x GNOME).

É preciso um produto novo — não um port — cuja arquitetura assuma essas regras.

## 3. Objetivos e não-objetivos

**Objetivos**

- O1: reservar faixa à direita via layer-shell (`exclusive_zone`), com área útil
  reduzida de verdade (onde o compositor honrar).
- O2: UI do dock com divisões 1–3, menu, liberar/ocultar, configuração persistida.
- O3: encaixe das janelas na faixa por componente do compositor (regras geradas para
  Hyprland/sway; script KWin quando viável).
- O4: ser leve e sem root; config em `$XDG_CONFIG_HOME/mayerdock/`.
- O5: comportamento previsível e documentado por compositor.

**Não-objetivos (MVP)**

- X11 (fora de escopo — decisão do ADR).
- GNOME/Mutter (exige extensão do Shell — projeto separado).
- Mover janelas alheias de forma genérica pelo app (impossível em Wayland).
- Bandeja (SNI) e empacotamento Flatpak/AppImage: fase seguinte.
- Overlay de escurecimento estilo FluxTV (exige efeito de compositor).

## 4. Público e cenários

**Público:** usuários de Wayland em **Hyprland/sway** (wlroots) e **KDE Plasma 6**
(KWin), que querem um painel fixo com apps lado a lado sem gerenciar regras à mão.

1. Abrir o MayerDock → dock aparece à direita; area útil encolhe (exclusive zone).
2. Escolher 2 divisões e encaixar navegador em cima e editor embaixo.
3. Maximizar outro app → termina na borda do dock.
4. Fechar o dock → espaço é devolvido ao compositor.
5. Reiniciar a sessão → configuração mantida.

## 5. Escopo — MVP Wayland (P0)

| ID | Requisito | Critério de aceite |
| --- | --- | --- |
| RF-L01 | Detectar sessão Wayland e abortar com mensagem clara fora dela | `XDG_SESSION_TYPE=wayland` (ou `WAYLAND_DISPLAY`); erro amigável em X11 |
| RF-L02 | Criar layer-surface ancorada à direita (`top`), altura total | Superfície aparece à direita do monitor escolhido |
| RF-L03 | Reservar faixa com `set_exclusive_zone(largura)` | Compositor reduz área útil; janela maximizada para na borda (validar em sway/Hyprland) |
| RF-L04 | Largura ajustável (arrastar borda ou config) 240–1200 px | Área reservada acompanha |
| RF-L05 | Divisões 1–3 configuráveis, com separadores e rótulos | Menu permite alternar; divisões ficam salvas |
| RF-L06 | Menu próprio na superfície (escolher janela, divisões, liberar, sair) | Menu abre/fecha por mouse/teclado (`Esc`) |
| RF-L07 | Persistir config (`$XDG_CONFIG_HOME/mayerdock/config.toml`, largura + divisões + output) | Reabrir mantém |
| RF-L08 | Gerar/atualizar regras de encaixe para Hyprland/sway | Arquivo de regras gerado + instrução de inclusão (`source`/`exec`) |
| RF-L09 | Liberar janelas (voltar ao estado anterior) | Regra removida e janela volta ao layout normal do compositor |
| RF-L10 | Instância única por sessão (lockfile/socket) | Segunda execução foca/sai |
| RF-L11 | Sair restaura a área útil | exclusive zone removida ao fechar |
| RF-L12 | Log em `$XDG_STATE_HOME/mayerdock/mayerdock.log` | Início, erros e comandos registrados |

## 6. Fora do MVP (backlog)

- **P1:** script KWin (JS) para encaixe no Plasma 6; IPC D-Bus entre dock e script;
  escolher monitor/output no menu; atalho global (via compositor).
- **P2:** bandeja StatusNotifierItem (`ksni`); AppImage/Flatpak/.deb; tema claro;
  divisões desiguais; perfis por app.
- **P3:** extensão GNOME Shell; experimento X11; overlay de escurecimento.

## 7. Requisitos não funcionais

- **RNF-01:** leve — sem daemons extras; binário único + config (GTK4 empacotado ou
  dependência de sistema).
- **RNF-02:** sem root; config/estado no padrão XDG.
- **RNF-03:** Wayland-only; funcionar em Hyprland e sway (wlroots) e, com ressalvas,
  KDE Plasma 6.
- **RNF-04:** robusto a restart do compositor (reconectar layer-surface).
- **RNF-05:** build offline quando possível; dependências de sistema documentadas.

## 8. Arquitetura (resumo)

```text
mayerdock-wayland (Rust + GTK4/gtk4-layer-shell)
   │  layer-surface à direita, exclusive_zone = largura
   │  UI do dock (divisões, menu) + config
   │
   └── gera/atualiza regras de encaixe:
         • Hyprland: windowrulev2 (size/move) + exec-once
         • sway: for_window [app_id=...] resize set/move position
         • KWin (P1): script lê config e posiciona janelas
```

Detalhes e justificativas no [ADR-001](ADR-001-linux.md).

## 9. Riscos

| Risco | Mitigação |
| --- | --- |
| KWin não honrar exclusive zone na versão alvo | Validar cedo; fallback overlay + regras (sem reserva real) |
| Regras de compositor quebrarem entre versões | Isolar por compositor, versionar e documentar |
| GTK4 + layer-shell no sandbox (Flatpak) | AppImage primeiro; Flatpak com permissões adequadas depois |
| Expectativa "igual ao Windows" (encaixe automático) | Comunicar: encaixe é do compositor, não do app |
| Fragmentação Wayland | MVP em wlroots; KDE como alvo secundário validado |

## 10. Critérios de aceite do MVP

1. Em **sway** e **Hyprland**, abrir o dock reduz a área útil (exclusive zone) e uma
   janela maximizada para na borda do dock.
2. Alternar 1/2/3 divisões funciona e persiste após reiniciar o app.
3. Escolher uma janela gera a regra correspondente e o compositor a posiciona na divisão.
4. Liberar remove a regra e a janela volta ao layout normal.
5. Fechar o dock devolve o espaço (área útil volta ao normal).
6. Segundo lançamento não duplica o dock.
7. Log e config nos caminhos XDG.

## 11. Roadmap

1. **Fase 0:** esqueleto layer-shell + exclusive zone (sway/Hyprland) + config.
2. **Fase 1:** UI (divisões, menu, liberar) e persistência.
3. **Fase 2:** gerador de regras Hyprland/sway + validação de encaixe.
4. **Fase 3:** KWin script + empacotamento (AppImage).
5. **Fase 4:** bandeja SNI; avaliar GNOME (extensão) e X11 (experimento).

## 12. Decisões registradas

- **Wayland apenas** (X11 fora de escopo).
- **Projeto separado** do MayerDock Windows, com núcleo de configuração/modelo em
  `mayerdock-core`.
- **Encaixe é responsabilidade do compositor** (regras/script) — o app não movimenta
  janelas alheias.
- UI: GTK4 + `gtk4-layer-shell` no MVP (alternativa `iced_layershell` se a estética
  pesar mais que menus prontos).
