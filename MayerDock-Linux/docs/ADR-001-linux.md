# ADR-001 — Portabilidade do MayerDock para Linux (Wayland)

- **Status:** Proposto
- **Data:** 2026-11-06
- **Escopo:** portabilidade do MayerDock (Windows) para Linux, **somente Wayland**
- **Projetos afetados:** `MayerDock` (Windows, permanece como está) e `MayerDock-Linux` (novo)

---

## Contexto

O MayerDock Windows é um AppBar: registra uma faixa junto ao shell (`SHAppBarMessage`),
o Windows encolhe a área de trabalho e as janelas maximizadas param na borda do dock.
Além disso, o app **movimenta e mantém janelas de outros processos** dentro da faixa
(Nível A), as promove a `HWND_TOPMOST`, esconde tudo em fullscreen, usa bandeja
(`Shell_NotifyIcon`), gancho de foreground (`SetWinEventHook`) e GDI para a UI.

No Linux/Wayland esse modelo não existe de forma genérica:

| Necessidade | Windows (atual) | Wayland |
| --- | --- | --- |
| Reservar faixa da área útil | `SHAppBarMessage` (`ABM_NEW/SETPOS`) | `zwlr_layer_shell_v1` + `zwlr_layer_surface_v1.set_exclusive_zone()` — o compositor pode ou não honrar |
| Sempre no topo | `HWND_TOPMOST` | camada `top`/`overlay` do layer-shell; clientes normais **não** têm "always on top" |
| Mover/redimensionar janela alheia | `SetWindowPos(hwnd_alheio)` | **impossível** — o protocolo Wayland não permite um cliente manipular outro; só o compositor |
| Ficar fora do Alt+Tab | `WS_EX_TOOLWINDOW` | consequência do layer-shell (não entra no task switcher) |
| Ícone na bandeja | `Shell_NotifyIcon` | StatusNotifierItem/AppIndicator (GNOME precisa de extensão) |
| Saber quando há fullscreen | `SHQueryUserNotificationState` | o compositor decide; não há consulta portátil |
| Capturar foco global | `SetWinEventHook` | sem equivalente para clientes comuns |

O ambiente Wayland é fragmentado por compositor:

- **wlroots** (sway, Hyprland, river, Wayfire): layer-shell completo, inclusive
  exclusive zone e posicionamento de janelas via IPC/configuração;
- **KWin (KDE Plasma)**: suporte a layer-shell em versões recentes (validar exclusive
  zone na versão alvo); posicionamento de janelas via scripts KWin/Window Rules;
- **Mutter (GNOME)**: **não** implementa layer-shell; qualquer dock de terceiro exige
  **extensão do GNOME Shell** (JS) — outro produto, fora do app Rust.

Também é relevante: o protocolo layer-shell (`wlr-layer-shell`) é o padrão de facto dos
painéis/barras (Waybar, sfwbar, etc.), mas **não faz parte do Wayland core** — é uma
extensão de compositor.

---

## Decisão

1. **Projeto separado (`MayerDock-Linux`)**, dentro do repositório do MayerDock mas como
   crate/projeto Rust independente. Não haverá binário único cross-platform nem `cfg`
   espalhado pelo código Win32.
2. **Alvo exclusivo: Wayland.** X11 fica explicitamente fora de escopo (mesmo sendo mais
   fácil de replicar).
3. **Arquitetura em duas partes**, porque um cliente Wayland não move janelas de outros
   apps:
   - **Parte 1 — dock (cliente Wayland):** janela layer-shell ancorada à direita, com
     `exclusive_zone` = largura do dock, UI do dock (divisões 1–3, menu, liberar) e
     persistência de configuração. É o equivalente ao AppBar.
   - **Parte 2 — encaixe (lado do compositor):** a colocação de janelas na faixa é feita
     por um componente do compositor:
     - **KDE/KWin:** script/regra KWin que lê a geometria do dock (arquivo de config ou
       D-Bus) e posiciona as janelas escolhidas nas divisões;
     - **Hyprland/sway:** geração de regras (`windowrulev2`, `for_window`) a partir da
       configuração do dock + instruções de instalação das regras;
     - **GNOME:** fora do MVP; extensão do GNOME Shell como projeto separado.
4. **Núcleo compartilhado mínimo:** `mayerdock-core` (modelo de divisões, geometria,
   configuração `.ini`/TOML, comandos) em Rust puro, usado pelo dock Wayland e,
   futuramente, por outras plataformas. Sem dependência de toolkit.
5. **UI do dock:** Rust + **GTK4** com `gtk4-layer-shell` (bindings do protocolo), por já
   trazer menus, fontes e acessibilidade. Alternativa avaliada: `iced` + `iced_layershell`
   (mais controle/estética, menos pronto para menus); decide-se por GTK4 no MVP.
6. **Bandeja:** fora do MVP. Em fase seguinte, StatusNotifierItem (ex.: crate `ksni`),
   com ressalva de que GNOME exige extensão de AppIndicator.

---

## Consequências

**Positivas**

- Não contamina o projeto Windows: cada um tem build, dependências, instalador e CI
  próprios.
- O núcleo (`config`/modelo de divisões) é reaproveitável e testável sem GUI.
- Ao assumir Wayland-only, o desenho fica correto (layer-shell) em vez de tentar imitar
  Win32 com gambiarras.
- Em compositores wlroots, o resultado pode ficar mais fiel que no Windows: exclusive
  zone + regras nativas.

**Negativas / custos**

- **Não é possível** reproduzir o "Nível A" de forma genérica: sem componente do
  compositor, o dock só reserva espaço; encaixar janelas é tarefa do compositor/regras.
- Cobertura por compositor é desigual: wlroots (bom), KWin (validar), GNOME (inviável
  sem extensão).
- Duas bases de código com comportamentos diferentes (documentar claramente).
- UI e empacotamento mudam: AppImage/Flatpak/.deb no lugar de Inno Setup; bandeja SNI
  no lugar de `Shell_NotifyIcon`.

**Neutras**

- O nome/UX continuam os mesmos (dock à direita, divisões verticais, menu).
- A versão Windows segue como produto principal; Linux é experimental até validar em
  KDE e wlroots.

---

## Alternativas consideradas

| Alternativa | Por que não |
| --- | --- |
| **Port X11 primeiro** (`_NET_WM_STRUT_PARTIAL` + EWMH para mover janelas) | Seria mais fácil e permitiria "Nível A" de verdade, mas o foco do produto é Wayland; X11 fica como possível experimento futuro. |
| **Binário único com `cfg(target_os)`** | ~96% do código atual é Win32; `cfg` espalhado pioraria a manutenção e o build (dependências gráficas no Windows). |
| **Electron/Tauri com plugin de layer-shell** | Já existe `tauri-plugin-*`? Nenhum plugin maduro de layer-shell; WebView não é layer-shell e o binário fica pesado. |
| **Forkar Waybar/outro painel** | Waybar é uma barra de status, não um dock com slot para janelas; adaptar seria mais trabalho que escrever o dock. |
| **Só gerar configuração do compositor** (sem UI) | Não entrega o produto (dock visual + menu); pode virar complemento (opção "modo leve"). |
| **GNOME primeiro** | Mutter não tem layer-shell; exigiria extensão JS — outro produto, outra linguagem. |

---

## Fases

1. **Fase 0 — esqueleto Wayland** (sway/Hyprland): crate layer-shell ancorado à direita,
   exclusive zone configurável, aparecer/desaparecer, config em disco.
2. **Fase 1 — UI do dock**: divisões 1–3, menu próprio (área do dock), liberar/recolher,
   persistência.
3. **Fase 2 — encaixe via compositor**: script KWin + gerador de regras Hyprland/sway;
   IPC (D-Bus ou arquivo) entre dock e script.
4. **Fase 3 — bandeja SNI + empacotamento** (AppImage/Flatpak) e testes em KDE Plasma 6.
5. **Fase 4 (opcional)**: extensão GNOME Shell; X11 como experimento separado.

---

## Riscos

| Risco | Mitigação |
| --- | --- |
| KWin não honrar `exclusive_zone` na versão do usuário | Validar cedo em Plasma 6; fallback: overlay + regras de janela (sem reserva real) |
| Protocolo layer-shell ser "não-padrão" | É o padrão de facto; aceitar dependência (como Waybar/Ignis) |
| Script de compositor quebrar entre versões (KWin API) | Isolar em `compositor/kwin` versionado; documentar versão suportada |
| GNOME ficar de fora | Documentado como não-objetivo; extensão em fase separada |
| Expectativa de "igual ao Windows" | README/PRD deixam claro: sem mover janela alheia de forma genérica |

---

## Questões em aberto

- Compositor alvo do MVP: **KDE Plasma 6** (mais gente) ou **Hyprland/sway** (wlroots,
  protocolo mais previsível)? Recomendação: validar o esqueleto nos dois e escolher a UI
  principal depois.
- Encaixe: script KWin (JS) x regras manuais documentadas (menos código, menos UX).
- UI: GTK4+layer-shell x `iced_layershell` — decidir após o protótipo do layer surface.
- Empacotamento: Flatpak (isolamento, mas permissões de layer-shell?) x AppImage (simples).

## Referências

- `wlr-layer-shell-unstable-v1` (protocolo de camadas/exclusive zone).
- EWMH `_NET_WM_STRUT_PARTIAL` (equivalente X11, fora de escopo).
- `gtk4-layer-shell`, `iced_layershell` (opções Rust de UI em layer-shell).
- KWin scripting / Window Rules (KDE); `windowrulev2` (Hyprland); `for_window` (sway).
- StatusNotifierItem (bandeja em Wayland).
