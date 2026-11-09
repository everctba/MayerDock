# MayerDock-Linux

Versão Linux do MayerDock — **exclusivamente Wayland**. Projeto separado do MayerDock
(Windows) porque a implementação não porta: apenas o modelo de produto (faixa reservada,
divisões, menu, configuração) é compartilhado conceitualmente.

> Decisão de arquitetura registrada em [docs/ADR-001-linux.md](docs/ADR-001-linux.md).
> Escopo de produto em [docs/PRD.md](docs/PRD.md).

## Por que separado

O MayerDock Windows tem ~1.900 linhas, das quais ~96% são Win32 (AppBar, `SetWindowPos` em
janelas de outros processos, `Shell_NotifyIcon`, GDI, ganchos de acessibilidade). No
Wayland **não existe equivalente** para várias dessas operações:

- reservar faixa da tela → `zwlr_layer_shell_v1` + `set_exclusive_zone` (compositor decide
  se honra; wlroots honra, KWin/KDE recente tem suporte parcial);
- "sempre no topo" → camada `top`/`overlay` do layer-shell (não é `HWND_TOPMOST`);
- mover/encaixar janela de outro app → **impossível para um cliente Wayland**; precisa ser
  feito pelo compositor (script/regra do KWin, `windowrulev2` do Hyprland, `for_window`
  do sway) ou extensão (GNOME);
- bandeja → StatusNotifierItem/AppIndicator (GNOME precisa extensão).

## Status

**Proposto / não iniciado.** O próximo passo é o esqueleto do projeto (protocolo de
camada + reserva de faixa) validado em sway/Hyprland (wlroots) e KWin.

## Estrutura prevista

```text
MayerDock-Linux\
├─ docs\
│  ├─ ADR-001-linux.md   # decisão: por que Wayland-only e por que projeto separado
│  └─ PRD.md             # escopo do MVP Wayland
├─ crates\
│  ├─ mayerdock-core\    # modelo de slots/configuração (puro Rust, compartilhável)
│  └─ mayerdock-wayland\ # cliente layer-shell (UI do dock)
└─ compositor\
   ├─ kwin\              # script/regra KWin para posicionar janelas na faixa
   └─ scripts\           # geradores de regras para Hyprland/sway
```
