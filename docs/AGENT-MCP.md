# AGENT-MCP — workspace de agentes de IA no MayerDock (proposta de produto)

**Versão do documento:** 0.1 (conversa/design) · **Data:** 2026-11-24
**Alvo de entrega:** **MayerDock v1.5** · **Estado atual:** v0.1.0 (DPI + MSIX F1/F2)
**Escopo aqui:** produto e arquitetura. **Sem código** — implementação em tasks no beads.

---

## 1. Visão

> **O MayerDock dá um workspace para seus agentes de IA — sem sequestrar o seu PC.**

Hoje os harnesses (Claude Code, Cursor, DSH, etc.) abrem browser e executáveis na cara do
usuário, roubam foco, movem o mouse e atrapalham o trabalho. A proposta é o MayerDock virar
a **camada de apresentação e controle** entre o agente e as janelas que ele precisa operar:

- as janelas do agente nascem **encaixadas em slots do dock**, sem ativar e sem mover o
  cursor real;
- o agente **captura só a janela dele** (nunca a tela inteira, nunca as suas janelas);
- o agente **age por input direcionado** (UI Automation/CDP/PostMessage) em vez de
  `SendInput` global;
- quando o app **exige** foco real, o agente pede um **lease** com **modal de aprovação**;
- tudo aparece num **HUD no dock** ("Claude: testando checkout… 2 cliques, 1 screenshot")
  com **botão Parar**.

O MCP é o contrato: qualquer harness que fale **MCP stdio** ganha essas capacidades.

---

## 2. Decisões fechadas (2026-11-24)

| # | Decisão |
| --- | --- |
| D1 | Transporte: **MCP stdio** (servidor local por harness; opcionalmente HTTP local para multi-agente). |
| D2 | A dor é **browser e app nativo** — os dois entram no escopo. |
| D3 | **Focus lease permitido**, sempre com **modal de aprovação do usuário** (e opção de negar/lembrar). Default do sistema continua "não roubar foco". |
| D4 | **Windows 10 19041** é o alvo mínimo (define APIs: WGC desde 1803, UIA, PrintWindow, AppBar, MSIX). |
| D5 | **HUD no dock** é requisito de produto **e** ativo de marketing da landing page. |
| D6 | Features novas vão para a **v1.5** no GitHub; a versão atual (0.1.0) vira **marco** (tag/release). |
| D7 | Sem autostart (decisão anterior mantida). |

---

## 3. Princípios de design

1. **Você é o dono do mouse e do foco.** Por padrão o agente não usa `SendInput`, não ativa
   janelas e não captura a tela inteira.
2. **Captura e ação direcionadas.** Screenshot de janela específica; clique/digitação
   endereçados ao controle (UIA) ou à janela (PostMessage/CDP).
3. **Visibilidade total.** O usuário vê o que o agente está fazendo (HUD, thumbnails,
   timeline) e pode parar em um clique.
4. **Privacidade por padrão.** O agente só vê o que está autorizado a ver; janelas do
   usuário são invisíveis para ele salvo permissão explícita.
5. **Degradação honesta.** O que não dá para fazer sem foco (alguns apps) fica bloqueado
   atrás de lease ou de isolamento — não "hackeado" silenciosamente.
6. **Local e rápido.** Sem VM por padrão; um pipe local e chamadas de milissegundos.

---

## 4. O que é fisicamente possível (e o que não é)

| Necessidade | Sem roubar seu mouse | Observação |
| --- | --- | --- |
| Abrir app/browser e ver | ✅ | Janela dockada com `SWP_NOACTIVATE`; captura WGC/PrintWindow |
| Screenshot de janela | ✅ | WGC (GPU) ou PrintWindow (GDI); funciona com janela **ocluída** |
| Clicar/digitar em app nativo | ⚠️ 60–80% | UIA `Invoke/SetValue` e PostMessage; Chromium/Electron exigem CDP |
| App que exige foreground | ⚠️ | Só com **lease** (modal) ou isolamento (desktop/sessão/VM) |
| Dois cursores ao mesmo tempo | ❌ | Um desktop = um cursor; isolar (VM/sessão) resolve |
| WebGPU/WebGL2 real | ⚠️ | Precisa de GPU real → **headed**, não headless (ver §8) |

**Regra de ouro:** o produto não é "mouse remoto"; é **workspace + captura + input
direcionado**, com escape hatch (lease/isolamento).

---

## 5. Arquitetura

```
harness (Claude Code / Cursor / DSH)
        │  MCP stdio (JSON-RPC)
        ▼
mayerdock-mcp.exe  ── named pipe (token por sessão) ──►  MayerDock (UI thread)
   │  tools                          │
   ├─ window mgmt / dock slots       ├─ AppBar, slots, HUD
   ├─ captura (WGC/PrintWindow)      ├─ janelas encaixadas
   ├─ UIA / PostMessage / CDP        └─ permissões, auditoria
   └─ devolve PNG + caminho + árvore
```

- **Sidecar**, não dentro do processo do dock: um bug de MCP não derruba o dock.
  `mayerdock.exe --mcp` pode ser só um alias que sobe o sidecar.
- **Named pipe** `\\.\pipe\MayerDock.Agent` com ACL do usuário + **token por sessão**.
- Comandos executados na **thread da UI** (mensagens), DPI já resolvido pela v0.1.0.
- Capturas em `%TEMP%\MayerDock\agent\<sessão>\` (arquivo + conteúdo MCP image).
- Sem rede; sem elevação.
- Multi-agente: 1 conexão = 1 sessão; sessões têm slots e escopos próprios.

---

## 6. Ferramentas MCP

### v1.5 (MVP)

| Tool | Função |
| --- | --- |
| `list_windows` | Janelas (título, classe, PID, retângulo, slot, dono/sessão) |
| `list_slots` | Slots do dock, ocupação e geometria |
| `open_app(path, args?)` | Abre **encaixado**, sem ativar (`SWP_NOACTIVATE`) |
| `capture_window(id)` | PNG da janela (WGC com fallback PrintWindow) |
| `capture_region(id, rect)` | Recorte da janela |
| `accessibility_tree(id)` | Árvore UIA em JSON/texto — agente "lê" sem visão |
| `invoke(id, selector)` | UIA Invoke/Toggle/Select |
| `click(id, x, y)` | Clique direcionado (UIA quando possível; PostMessage; nunca global por padrão) |
| `type_text(id, text)` | Digitação direcionada (UIA ValuePattern / PostMessage) |
| `press_key(id, combo)` | Tecla direcionada |
| `wait_for(id, condição)` | Título mudou / pixel / UIA / idle (sem polling cego) |
| `window_state(id, ação)` | Mover/redimensionar/maximizar/minimizar/encaixar em slot |
| `focus_lease(id, segundos, motivo)` | Abre **modal** no dock pedindo OK do usuário |
| `browser_launch(profile, flags, posição)` | Sobe Chromium/Edge com perfil dedicado e flags anti-throttle |
| `status` / `session_close` | Estado da sessão; encerrar e liberar slots |

### Fase 2 (pós-1.5)

`record_session` / `replay`, `diff_visual`, `screen_capture_all` (permissão separada),
`virtual_monitor` (display virtual), `isolated_session` (desktop/sessão/VM), `multi_agent`
(fila/prioridade), `agent_cursor` (cursor sintético desenhado nas capturas), `ocr`.

### Resources / prompts

- `mayerdock://windows`, `mayerdock://slots/{id}/screenshot`, `mayerdock://sessions`
- Guia embutido entregue ao agente: *"prefira árvore UIA a pixels; nunca input global;
  screenshots só da janela autorizada; se precisar de foco, peça lease"*.
- Eventos: `window_opened/closed`, `session_started`, `lease_requested`.

---

## 7. Captura e input — decisões técnicas

- **Captura:** `Windows.Graphics.Capture` (WGC, 1803+) para janelas GPU/Chromium;
  `PrintWindow(PW_RENDERFULLCONTENT)` como fallback GDI. WGC **captura janela ocluída**
  (o DWM compõe mesmo fora da vista) — é o que nos permite testar "fora da tela".
- **DPI:** já resolvido na v0.1.0 (perMonitorV2 + métricas lógicas); capturar em pixels
  físicos e reportar `dpi`/`scale` ao agente.
- **Input:** ordem de preferência UIA → PostMessage → CDP (browser) → `SendInput` **só com
  lease**. Documentar por app o que funciona.
- **Janela oculta/minimizada:** WGC pode parar frames quando minimizada; para automação,
  manter a janela **não minimizada** (pode estar offscreen).
- **Anti-throttling:** ver §8.

---

## 8. Navegador: headless vs headed offscreen (e o depoimento técnico)

O depoimento de um usuário/agent resume o problema real:

> *"headless não tem WebGPU/WebGL2 confiável para o nosso caso (quebraria as checagens de
> backend e o baseline). Se um dia incomodar de verdade, dá para atenuar sem headless:
> `--disable-backgrounding-occluded-windows`, abrir o Chrome num perfil separado e/ou
> `--window-position=-2400,-2400` para a janela nascer fora da tela — mas isso pode
> reintroduzir o throttle de rAF (a gente já sofreu com janela ocluída)."*

**Diagnóstico:** headless (mesmo o "new headless") usa SwiftShader/software em muitos
casos; WebGPU/WebGL2 ficam instáveis ou com baseline diferente. Já headed com janela
offscreen/ocluída sofre **throttle de rAF** e backgrounding do Chromium.

**Como o MayerDock resolve (v1.5):**

1. **Modo headed offscreen gerenciado**: `browser_launch` sobe Chromium/Edge com **perfil
   dedicado** e flags que mantêm o pipeline de GPU real:
   - `--disable-backgrounding-occluded-windows`
   - `--disable-renderer-backgrounding`
   - `--disable-background-timer-throttling`
   - `--disable-features=CalculateNativeWinOcclusion` ← **a chave**: o Chrome deixa de
     calcular "janela ocluída" e não estrangula rAF por causa disso.
2. **Janela visível para o DWM, invisível para o usuário**: posicionada fora do monitor
   (`--window-position=-2400,-2400`) ou num **monitor virtual** (fase 2), **com captura
   WGC** — o frame continua real (GPU), sem depender de o usuário ver.
3. **Se nem isso bastar**: colocar a janela num **slot do dock** (visível mas sem foco) —
   o Chrome renderiza normalmente e o agente captura só aquele slot.
4. **Headless continua disponível** como opção (`browser_launch(headless=true)`) para
   fluxos que não precisam de WebGPU/WebGL2 — decisão do agente, com aviso no retorno.
5. **Fallback CDP**: Playwright/CDP continuam funcionando por cima; o MayerDock só cuida
   da janela, perfil e flags.

> Resumo para o marketing: *"headless quebra WebGPU/WebGL2; janela ocluída trava o rAF.
> O MayerDock roda o navegador **headed** com GPU real, do lado de fora da sua tela, sem
> travar o render e sem roubar seu foco."*

---

## 9. Níveis de isolamento

| Nível | Como | Rouba foco? | Quando usar | Fase |
| --- | --- | --- | --- | --- |
| **L0** | Slot dockado + captura direcionada + UIA/CDP | Não | Padrão (browser e apps que aceitam UIA/PostMessage) | v1.5 |
| **L1** | L0 + **focus lease** com modal | Breve, com OK | App que exige foreground | v1.5 |
| **L2** | Monitor virtual / janela offscreen + WGC | Não | Browser com GPU, testes visuais contínuos | pós-1.5 |
| **L3** | Desktop oculto / sessão / VM | Não (mundo separado) | Automação nativa completa, jogos/apps teimosos | pós-1.5 |

---

## 10. Focus lease + modal de aprovação

Fluxo:

1. O agente chama `focus_lease(id, 15s, "o campo de cartão só aceita tecla com foco")`.
2. O dock mostra um **modal** (janela topmost própria, no mesmo estilo do menu):
   *"Claude quer controlar o mouse/teclado de «Chrome — checkout» por 15 s. [Permitir] [Negar] [Sempre para este app]"*.
3. Se permitir: dock **trava** o input global por 15 s (hotkey `Esc` cancela), colore a
   borda e mostra contagem no HUD.
4. Ao terminar (ou no timeout), devolve o controle ao usuário e registra no log/auditoria.
5. "Sempre" vira regra por app/perfil nas configurações (revogável).

**Default:** lease **nunca** automático; expira; cancelável; o agente não pode pedir lease
em loop sem aprovação (rate limit).

---

## 11. HUD no dock + kill switch (requisito)

No cabeçalho do slot/janela do agente (e no topo do dock quando em execução):

```
● Claude  testando checkout…   2 cliques · 1 screenshot   [Parar]
● Cursor  rodando testes       12 ações                    [Parar]
```

- Nome do harness + atividade atual (o agente informa via `status`/eventos).
- Contadores (cliques, teclas, capturas), duração, e cor por sessão.
- **Botão Parar** por sessão e **Parar tudo** (kill switch global, hotkey
  `Ctrl+Alt+Shift+K`): cancela tools em andamento, libera leases, solta slots e mata
  processos lançados pela sessão (com confirmação se houver trabalho em andamento).
- Estado "aguardando aprovação" quando há modal/lease.
- Tudo em GDI (como o resto do dock), DPI-aware, sem dependências novas.

---

## 12. Permissões, segurança e privacidade

- **Escopos por sessão:** `windows:read`, `windows:manage`, `input:targeted`,
  `input:global` (lease), `screen:window`, `screen:all` (opt-in), `process:launch`.
- **Token** por sessão; pipe com ACL; sem rede por padrão.
- **Auditoria** (JSONL): toda tool, alvo, resultado, lease, negação.
- **Prompt injection:** conteúdo de tela/UIA é **dado**, nunca instrução; o guia embutido
  reforça; tools que devolvem texto marcam como untrusted.
- **Privacidade:** `capture_*` só do alvo; janelas do usuário fora do escopo; redigir
  títulos sensíveis; nunca capturar áreas de senha sem permissão.
- **Kill switch** sempre disponível (dock e hotkey).
- **Anti-abuso:** rate limit de lease (ex.: 3/min), escopo `input:global` só após
  aprovação explícita e nunca herdado entre sessões.

---

## 13. Multi-agente

- Várias sessões simultâneas (uma por harness), cada uma com seus slots.
- Dock mostra badges ("Claude", "Cursor") e o HUD agrega.
- Fila opcional: agentes que querem o mesmo app esperam lease/slot.
- Fechar sessão libera slots e janelas (com opção "manter aberta").

---

## 14. Performance e limites

- Captura: WGC contínua a 10–30 fps opcional; `capture_window` sob demanda (~10–40 ms).
- Latência de tool: pipe local + mensagem → **< 5 ms** de overhead típico.
- CPU parado: 0% (captura só sob demanda; HUD via timer leve).
- Memória: +2–6 MB para o sidecar (Rust) — dentro do orçamento atual (< 20 MB).
- Limite conhecido: apps que ignoram UIA/PostMessage precisam de lease (L1) — documentar
  por app.

---

## 14.1 Estado da implementação (2026-11-24)

Implementado:

- **sidecar MCP stdio** (`mayerdock.exe --mcp`): initialize/ping/tools/list/tools/call,
  guia embutido e sessão enviada ao dock;
- **dock bridge**: named pipe `\\.\pipe\MayerDock.Agent` com **token** por execução
  (`%APPDATA%\MayerDock\agent-token`), comandos executados na thread da UI via
  `SendMessage`; uma conexão (um agente) por vez;
- tools standalone: `status`, `list_windows`, `capture_window` (PrintWindow
  `PW_RENDERFULLCONTENT` + fallback BitBlt → PNG RGBA, DPI no retorno), `open_app`
  (`SW_SHOWNOACTIVATE`), `window_state`;
- tools do dock: `list_slots`, `dock_pin` (encaixa sem ativar), `dock_unpin`,
  `dock_release_all`;
- **HUD**: no cabeçalho do dock, `● <agente> · <atividade>` e botão **■ Parar**
  (kill switch) que encerra o processo do sidecar e derruba a sessão;
- **controles + input direcionado**: `list_controls` (EnumChildWindows), `click`
  (PostMessage WM_MOUSEMOVE/DOWN/UP, cliente ou tela), `type_text` (WM_CHAR),
  `send_key` (WM_KEYDOWN/UP), `get_text` (WM_GETTEXT) e `wait_for`
  (título/texto/visível com timeout);
- capturas em `%APPDATA%\MayerDock\agent\<pid>\cap-*.png`; log no `mayerdock.log`.

Validado end-to-end (dock + sidecar + Notepad): handshake, sessão no HUD, `list_slots`,
`open_app`, `dock_pin` (janela 704×517 no slot), captura PNG, `dock_release_all` e
**kill switch encerrando o sidecar**. Input: `list_controls` achou o `Edit` do Notepad,
`click` + `type_text` inseriram “MayerDock agent input test” (lido por `get_text` e
`wait_for`), **sem mover o cursor**.

**Focus lease (v2) validado:** `focus_lease` abre modal (`Permitir/Negar`) no dock; ao
aprovar, traz a janela para foco por N s, o HUD mostra `· lease Ns` e, ao expirar, restaura
a janela anterior. `global_click`/`global_type`/`global_key` (`SendInput`) **falham sem
lease** e funcionam durante o lease (digitou “global lease ok” no Notepad via mouse/teclado
reais). **“Permitir sempre para este app”** grava a regra em
`%APPDATA%\MayerDock\lease-allow.txt` (allowlist por processo, revogável apagando o
arquivo) e há **rate limit de 3 leases/minuto**. HUD ganhou contadores (`· 2c 1s`), botão
**■ Parar** por sessão, item “Parar agente” na bandeja e hotkey global
**Ctrl+Alt+Shift+K**. Para testes automatizados, `MAYERDOCK_LEASE_AUTO=1` aprova sem modal
(QA).

**Ações por mensagem e browser (v1.5):** `set_text` (`WM_SETTEXT`) e `control_click`
(`BM_CLICK` com fallback no centro) cobrem o essencial de apps Win32 sem UIA completo.
`browser_launch` sobe Chrome/Edge com **perfil por porta** (evita conflito de singleton),
`--remote-debugging-port`, `--user-data-dir` dedicado e as flags anti-throttle
(`--disable-backgrounding-occluded-windows`, `--disable-renderer-backgrounding`,
`--disable-background-timer-throttling`, `--disable-features=CalculateNativeWinOcclusion`);
headed usa `--window-position=-2400,-2400` (GPU real, fora da tela) e devolve o
**endpoint CDP** para o agente conectar via Playwright/CDP; headless é opcional com aviso
de WebGPU/WebGL2. O probe do endpoint usa `curl.exe` (o `TcpStream` do sidecar GUI dava
timeout no loopback).

**Empacotamento validado:** `.\build.ps1 -Installer -Msix` continua funcionando com o
sidecar — Inno 2,47 MB e MSIX 0,55 MB (exe 908 KB, deps offline). Corrigido bug do
`build.ps1` (`$installer` colidia com o switch `-Installer`, como antes com `$msix`).

**Próximo:** UIA COM completo (`IUIAutomation` `Invoke/SetValue`) e bridge CDP dedicada
(`md-gx2`), multi-agente (`md-298`) e WACK/Store.

---

## 15. Roadmap

### v0.1.0 — marco atual (concluído)
DPI per-monitor, migração `width_logical`, app manifest (PerMonitorV2/asInvoker),
`TaskbarCreated`, `--status`, MSIX F1/F2 (build/assinatura/sideload/upgrade).
→ **Task de marco + tag/release no GitHub.**

### v1.5 — Agent Workspace (MVP)
Escopo: L0 + L1, HUD/kill switch, MCP stdio sidecar, tools v1, captura WGC/PrintWindow,
UIA + PostMessage + CDP bridge, permissões/auditoria, `browser_launch` anti-throttle,
docs + guia embutido, configurações no dock (abas: Agentes).
Fora do 1.5: L2/L3, record/replay, OCR, multi-agente avançado.

### Pós-1.5
Record/replay, diff visual, monitor virtual, desktop oculto/sessão isolada, OCR,
marketplace de "receitas" de testes.

---

## 16. Marketing / landing page (conteúdo aprovado)

**Posicionamento:**
> **MayerDock — o dock que dá um workspace para seus agentes de IA sem sequestrar seu PC.**

**Hero (sugestões):**
- "Seu agente trabalha. Você continua trabalhando."
- "Nada de janela na sua cara, cursor pulando ou print da sua tela inteira."

**Prova visual (HUD) — ativo central:**
`● Claude testando checkout… 2 cliques · 1 screenshot [Parar]`
GIF/vídeo: você digitando no editor à esquerda; à direita o dock com a janela do agente,
HUD mudando, você clica **Parar** e o agente para na hora.

**Comparativo:**

| | Playwright MCP | windows-mcp/UIA | Computer-use em VM | **MayerDock Agent** |
| --- | --- | --- | --- | --- |
| Browser | ✅ | parcial | ✅ | ✅ (CDP + janela gerenciada) |
| App nativo | ❌ | ✅ UIA | ✅ | ✅ UIA/PostMessage + lease |
| Não rouba seu foco | parcial | ❌ | ✅ (VM) | ✅ **por padrão** |
| Sem VM, local e rápido | ✅ | ✅ | ❌ | ✅ |
| Você vê e para | ❌ | ❌ | ❌ | ✅ **HUD + kill switch** |

**Credibilidade técnica (conteúdo de blog):**
- "Por que headless quebra WebGPU/WebGL2" (SwiftShader vs GPU real; baseline diferente).
- "Throttle de rAF em janela ocluída e o `CalculateNativeWinOcclusion`".
- "Como capturamos uma janela que você não vê (WGC) sem roubar foco".
- "Focus lease: quando o agente precisa do mouse, ele **pede**".

**Store listing:** screenshots do HUD, descrição com "workspace para agentes", tag
"AI agents", justificativa do `runFullTrust` já preparada.

---

## 17. Riscos e mitigação

| Risco | Mitigação |
| --- | --- |
| Agente com input global causar dano | Default L0; lease só com modal; rate limit; kill switch; auditoria |
| Prompt injection via tela/UIA | Tela = dado; guia embutido; sem tool que execute instruções da tela |
| App que ignora UIA/PostMessage | Lease (L1) documentado; L3 no futuro |
| Throttle/headless (WebGPU/WebGL2) | `CalculateNativeWinOcclusion` off + perfil dedicado + WGC (ver §8) |
| MCP instável derrubar o dock | Sidecar separado; pipe com timeout |
| Store/review: "controla o mouse" | Posicionar como automação assistiva com permissões explícitas; escopos |
| Suporte por app | Tabela de compatibilidade + logs + "por que falhou" |

---

## 18. Decisões abertas

1. Nome do sidecar: `mayerdock-mcp` separado ou `mayerdock.exe --mcp`? (proposta: separado,
   com alias).
2. Multi-agente já no 1.5 ou depois? (proposta: 1 sessão por vez no 1.5, com fila).
3. Monitor virtual (L2) entra no 1.5 ou 2.0? (proposta: 2.0).
4. "Sempre permitir lease" por app expira em quanto tempo (30 dias? por sessão?).
5. Marketplace de receitas: 1.5 ou só landing page?

---

## 19. Referências

- MCP (Model Context Protocol) — spec stdio/HTTP.
- Playwright MCP / Chrome DevTools MCP — automação de browser via protocolo.
- Windows.Graphics.Capture (WGC) e PrintWindow(PW_RENDERFULLCONTENT) — captura de janela.
- UI Automation (IUIAutomation) — Invoke/Value/Select sem foco.
- Chromium flags: `CalculateNativeWinOcclusion`, `disable-backgrounding-occluded-windows`,
  `disable-renderer-backgrounding`, `disable-background-timer-throttling`.
- Beads (`bd`) — epics/tasks deste produto.
