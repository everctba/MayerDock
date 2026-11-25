# Backlog / tarefas — MayerDock

Tarefas futuras que **não** devem quebrar o que já funciona. O instalador atual
(Inno Setup, `build.ps1 -Installer`) é o caminho oficial de distribuição até a Store
estar pronta.

> **Backlog vivo no beads (`bd`):** as epics/tasks da próxima versão estão na base local.
> Use `bd list`, `bd ready`, `bd show <id>`.
>
> - **Marco atual:** v0.1.0 — DPI + MSIX F1/F2 (`md-ab7`).
> - **Marco v1.5:** Agent Workspace (`md-d33`).
> - **Epics v1.5:** Agent Workspace MCP (`md-jcf`) · Marketing/landing & Store (`md-o2l`)
>   · Release & versões (`md-9x9`).
> - Produto/arquitetura do Agent MCP: [AGENT-MCP.md](AGENT-MCP.md).

---

## T-001 — Build MSIX para Microsoft Store (em paralelo ao instalador atual)

- **Status:** pendente (futuro)
- **Prioridade:** P2
- **Objetivo:** gerar `dist\msix\MayerDock-x.y.z.msix` para publicação na Microsoft Store,
  **sem alterar** o instalador Inno nem o fluxo portátil.
- **Levantamento completo:** [MSIX.md](MSIX.md) — requisitos, riscos (V1–V8), insights e
  estimativa de esforço.

### Adaptações obrigatórias (resumo — detalhes no MSIX.md)

1. **Manifesto** `Package.appxmanifest` com `runFullTrust` (capability restrita —
   justificativa na Store) e `EntryPoint="Windows.FullTrustApplication"`.
2. **Autostart:** **fora de escopo (decidido em 2026-11-11)** — o app não tem "iniciar com
   o Windows" em nenhuma edição; nada de `HKCU\Run` nem `startupTask`.
3. **Dados:** ✅ validado — em full trust **não há redirecionamento**: config/log ficam no
   `%APPDATA%` real e **sobrevivem** ao uninstall. (Sem necessidade de Exportar/Importar
   para migrar edições.)
4. **Assets do pacote** (`Square150x150Logo`, `Square44x44Logo`, `StoreLogo`) gerados de
   `assets/icon.png`; `Identity Version` em **4 partes** (`0.1.0.0`).
5. **Assinatura:** sideload exige certificado confiável; a Store assina na publicação.
6. ✅ **App manifest Win32** embutido (`PerMonitorV2` + `asInvoker`) — **feito**.
7. ✅ **Detecção de modo pacote** (`GetCurrentPackageFullName`) + `--status` + re-registro
   do tray/AppBar em `TaskbarCreated` — **feito**.
8. **Não rodar Inno e MSIX ao mesmo tempo** (duas AppBars/trays).

### Requisitos / passos

1. ✅ **Manifesto** `installer\msix\Package.appxmanifest.template` → `AppxManifest.xml`
   gerado no staging (`Identity`/`Publisher` parametrizáveis; `runFullTrust`;
   `EntryPoint="Windows.FullTrustApplication"`; **feito**).
2. ✅ **Assets do pacote** em `installer\msix\Assets\` (logo 44/150/Store + targetsize do
   44), gerados de `assets/icon.png` — **feito**.
3. ✅ **Empacotamento** com `makeappx.exe` em `installer\msix\build-msix.ps1`
   (tooling via NuGet `Microsoft.Windows.SDK.BuildTools` 10.0.22621.x em `.tools\`) —
   **feito**; gera `dist\msix\MayerDock-0.1.0.msix` (0,32 MB).
4. ✅ **Flag isolada no `build.ps1`**: `-Msix` (não mexe em `-Installer`, `-Run`,
   `-Shortcut`) — **feito**.
5. **Sideload local**: `-SelfSign` cria certificado `CN=MayerDock Dev` em `CurrentUser`,
   assina e confia em `TrustedPeople`; **falta** testar instalação — o Windows exige a
   **raiz** confiável (`0x800B0109`), o que nesta máquina precisa de uma ação de admin
   (`certutil -addstore TrustedPeople dist\msix\mayerdock-dev.cer`) ou Modo de
   Desenvolvedor. Pendente de validação manual.
6. **Store**: reservar o nome no Partner Center e subir o `.msix`/`.msixupload`
   **sem assinatura** (a Store assina). Ajustar `-IdentityName`/`-Publisher`.

### Critérios de aceite

- [ ] `.\build.ps1 -Msix` gera `dist\msix\MayerDock-x.y.z.msix` **sem** tocar em
      `dist\installer\` nem em `dist\MayerDock\`.
- [ ] `.\build.ps1 -Installer` continua funcionando exatamente como hoje.
- [ ] O pacote instalado por sideload abre, aparece em "Aplicativos instalados", cria o
      ícone da bandeja, reserva a faixa (AppBar) e encaixa janelas.
- [ ] Desinstalar remove o pacote e restaura a área de trabalho.
- [x] `--status`/log mostram versão, modo pacote e caminho real de config/log — **feito**.
- [x] App manifest Win32 (`PerMonitorV2`/`asInvoker`) embutido — **feito**.
- [x] Re-registro de bandeja/AppBar após restart do Explorer (`TaskbarCreated`) — **feito**.
- [x] Sem autostart (decisão): nada a implementar.
- [x] Sideload instalado e ativado (`empacotado=true`); AppBar/encaixe/bandeja OK
      (V1/V2/V3) — **feito em 2026-11-11** (Developer Mode + cert em LM\TrustedPeople).
- [x] Desinstalação limpa e config preservado (V7) — **feito**.
- [x] Upgrade 0.1.0 → 0.1.1 com app fechado: config preservado (V6) — **feito**;
      com app aberto falha com `0x80073D02` (esperado; Store gerencia).
- [x] V4: hook de foreground (Start/pesquisa) e hotkey `Ctrl+Alt+M` funcionam no pacote — **feito**.
- [x] V8: Inno + MSIX simultâneos não duplicam dock (instância única) — **feito**.
- [ ] V5: detecção de fullscreen real (jogo/vídeo) — pendente de teste manual.
- [ ] WACK aprovado.
- [ ] Documentar em `docs\MSIX.md` como gerar, instalar e publicar.

### Riscos

| Risco | Mitigação |
| --- | --- |
| `runFullTrust` ausente quebra AppBar/controle de janelas | Declarar no manifesto desde o início; testar sideload |
| MSIX exige Windows 10 1809+ | Documentar requisito; Inno continua para versões antigas |
| SDK do Windows ausente na máquina de build | Script detecta e orienta a instalação; não bloqueia o build atual |
| Identidade da Store muda o `Identity` | Só fixar o `Identity` após reservar o nome no Partner Center |
| Atualização automática da Store substituir a versão portátil | São produtos distintos (MSIX vs instalador/portátil); documentar |

---

## T-002 — Fuzzing do MayerDock (MCP/JSON, named pipe, config.ini, CLI, captura)

- **Status:** em progresso (fuzz-lite implementado) · **Prioridade:** P2 · **Task no beads:** `md-d33.1`
- **Objetivo:** fuzzar as superfícies que recebem dados **não confiáveis** para encontrar
  panics, crashes, DoS e uso descontrolado de memória — sem quebrar os builds atuais.

### Implementado (fuzz-lite, 2026-11-24)

- Parsers puros extraídos e cobertos por testes: `config::parse_ini`,
  `mcp::parse_message_line` + `mcp::read_line_capped`, `dock::decode_pipe_line`.
- Testes em `cargo test` (`.\build.ps1 -Test`): corpus de extremos + ~60 mil entradas
  aleatórias (deep nesting, bytes inválidos/NUL, 1 MB, oversized), sem panic.
- `scripts/fuzz_smoke.py`: manda corpus hostil + linha de **5 MB** + JSON profundamente
  aninhado + UTF-8 inválido no `--mcp` e valida que o sidecar **sobrevive e responde**
  (`tools/list` = 24 tools). Rodado com sucesso.
- Correção: leitura do stdin do MCP agora tem **limite de 4 MB por linha** (descarta o
  excesso sem crescer memória); pipe do dock documentado/testado com limite de 1 MB.
- Ainda pendente: alvos com `cargo-fuzz`/libFuzzer (bloqueado por toolchain GNU/offline)
  e rodadas de 10 min por alvo em CI.

### Alvos (ordem de risco)

1. **MCP stdio** (`src/mcp.rs`): JSON malformado, linhas > 1 MB, tipos errados, `id`
   ausente/duplicado, unicode inválido, aninhamento profundo, `tools/call` com
   `arguments` absurdos (`hwnd` inválido, `text` gigante, `title` com `%`/NUL).
2. **Named pipe do dock** (`src/dock.rs`): auth sem token/errado/gigante, request sem
   JSON, linha sem `\n`, payload > 1 MB, desconexão no meio, requests fora de ordem,
   `WM_AGENT_REQ` malformado, sessão sem `pid`.
3. **config.ini** (`src/config.rs`): chaves desconhecidas, `width` em `i32::MIN/MAX`,
   `slots` 0/999, `legacy_width` negativo, linha truncada, CRLF/LF/BOM, unicode.
4. **CLI/env:** `--mcp`/`--status`/`--version` com args extras, `MAYERDOCK_DPI` com
   0/99999/negativo/texto, `MAYERDOCK_LEASE_AUTO` com valores estranhos.
5. **Captura/PNG:** `hwnd` inválido/destruído no meio, janela 0×0, `GetDIBits` falhando,
   dimensões negativas/gigantes — nenhum panic pode derrubar dock/sidecar.

### Estratégia

- Extrair os **parsers puros** (MCP/pipe/config/CLI) para módulos testáveis.
- Harness de fuzz: `cargo-fuzz`/libFuzzer se houver toolchain MSVC e rede para
  `libfuzzer-sys`; alternativas: `cargo-afl` em WSL, `honggfuzz`, ou harness Rust com
  corpus + geração aleatória via `std` (o cache offline atual só tem windows-sys,
  serde_json, base64, png).
- Corpus inicial: mensagens MCP reais, configs reais e linhas de pipe; artefatos em
  `target/fuzz-artifacts` (ignorado no git).

### Critérios de aceite

- [ ] ≥ 3 alvos (`mcp_json`, `pipe_line`, `config_ini`) compilando e rodando com comando
      documentado.
- [ ] 10 min por alvo sem panic/abort/stack overflow e com RSS limitado (ex.: ≤ 200 MB).
- [ ] Mensagens > 1 MB e JSON com aninhamento profundo são rejeitadas sem travar
      dock/sidecar.
- [ ] Cada crash/DoS encontrado vira bug no beads com repro minimizado e correção.
- [ ] `docs/TASKS.md` e `docs/AGENT-MCP.md` atualizados com como rodar e corpus.
- [ ] Builds portátil, Inno e MSIX continuam passando após extrair os módulos.

### Notas e riscos

- Rodar com dock isolado e usuário idle; **nunca** fuzzar input global sem lease.
- Não capturar janelas de terceiros além das abertas pelo próprio teste.
- `cargo-fuzz` não funciona no toolchain GNU/offline desta máquina — documentar a
  alternativa usada e não bloquear o build atual.

---

## T-003 — Modo console/silent para CI (CLI JSON + script runner)

- **Status:** Fase 1 concluída · **Prioridade:** P1 · **Task no beads:** `md-d33.2`
- **Objetivo:** controlar o MayerDock por terminal/argumentos (sem abrir o dock), para
  esteiras de teste (ex.: DeepSeek Harness) validarem fluxos antes de tocar a conta real.

### Fase 1 — concluída (2026-11-24)

- Flags globais `--json` e `--silent`/`--quiet` (saída legível por máquina).
- `--status [--json]`, `--list-windows [--json]`, `--capture (--title|--hwnd) [--out] [--json]`.
- Códigos de saída: `0` ok, `1` erro de execução, `2` uso inválido.
- Testado no `.exe` de `dist\`: JSON válido, captura em arquivo, erro 1 para janela
  inexistente e erro 2 para argumento inválido.

### Fase 2 — script runner — concluída (2026-11-24)

`mayerdock.exe --script teste.json [--json] [--timeout 60]`.

```json
{
  "name": "checkout",
  "steps": [
    { "action": "open_app", "args": { "path": "notepad.exe" } },
    { "action": "wait_window", "args": { "title": "Notepad", "timeout_ms": 5000 }, "as": "np" },
    { "action": "type_text", "args": { "hwnd": "$np", "text": "hello" } },
    { "action": "assert_text", "args": { "hwnd": "$np", "contains": "hello" } },
    { "action": "capture", "args": { "hwnd": "$np", "out": "artifacts/np.png" } }
  ]
}
```

- Ações implementadas: `open_app`, `wait_window`, `find_control`, `click`, `type_text`,
  `send_key`, `set_text`, `control_click`, `get_text`, `wait_for`, `uia_query`,
  `uia_invoke`, `window_state`, `capture`, `assert_text`, `assert_window`, `list_slots`,
  `dock_pin`, `dock_unpin`, `dock_release_all`, `sleep`.
- Variáveis (`as`/`$var`), `continue_on_error`, timeout global/per-step, relatório JSON
  com pass/fail por passo, duração e artefatos; exit code 0/1/2 (BOM UTF-8 aceito).
- Validado: `scripts/sample_script.json` (Notepad: open → wait → find_control → set_text →
  assert_text → capture → close) passou em ~218 ms; script com assert errado retorna
  exit 1 e `aborted=true`; JSON inválido/arquivo ausente retorna exit 2.

### Fase 3 — integração CI — concluída (2026-11-24)

- `--ci`: saída JSON, sem texto humano, timeout padrão de 120 s e bloqueio de ações que
  dependem de interação (`focus_lease`/`global_*`); input direcionado segue permitido.
- `--report-junit ARQ`: XML `<testsuite>` com `tests`/`failures`/`time` e um
  `<testcase>` por passo (failure com a mensagem do assert).
- `--report-json ARQ`: relatório completo (steps, ms, output, error, duration).
- `--timeout ms`: limite global (além do `timeout_ms` por passo).
- Validado: script de sucesso com `--ci` → exit 0, JUnit `tests=7 failures=0`, JSON
  gravado; script com assert errado → exit 1 e JUnit `failures=1`; `--ci`/`--headless`
  sem comando e `--report-junit` sem `--script` → exit 2.
- **Limite:** automação de GUI exige sessão de desktop interativa — runners hospedados do
  GitHub Actions não têm; documentar self-hosted com autologon/RDP. Unit/fuzz
  (`build.ps1 -Test`) rodam em qualquer runner.

### Fase 4 — headless/no-UI — concluída (2026-11-24)

- `--headless`/`--no-ui`: aceito em conjunto com um comando de console e garante que o
  dock **nunca** abre (sem comando → exit 2 com mensagem de uso).
- Validado: `--script ... --headless` → exit 0; `--headless` sozinho → exit 2.

---

## T-004 — Diferenciais de UX do dock (pós-1.5 / backlog)

- **Epic no beads:** `md-d33.3` · **Prioridade:** P2/P3 · **Status:** backlog (não bloqueia
  a v1.5)
- **Objetivo:** transformar o dock de "organizador" em "workspace visível": layout salvo,
  slots desiguais, launcher, tema, cor de sessão e elementos de demo/streaming.

| Task (beads) | Feature | Prio | Resumo |
| --- | --- | --- | --- |
| `md-d33.3.1` | Workspaces por contexto | P2 | salvar/restaurar janelas+apps+layout por cenário (trabalho/game/reunião) |
| `md-d33.3.2` | Divisórias arrastáveis | P2 | slots desiguais (70/30), persistência e min/max |
| `md-d33.3.3` | Launcher no rodapé | P3 | favoritos, grupos e "abrir no slot X" |
| `md-d33.3.4` | Tema automático | P3 | claro/escuro/cor de destaque, acrílico opcional, snap animado |
| `md-d33.3.5` | Borda de foco por sessão | P2 | cor por agente/humano (demo/streaming) |
| `md-d33.3.6` | Badge na bandeja | P3 | sessão/lease/fila no ícone e tooltip |
| `md-d33.3.7` | Pair testing | P2 | "quem está no controle" no HUD + botão **Assumir** |
| `md-d33.3.8` | Guardrails | **P1** | bloquear apps sensíveis, redigir regiões/títulos, nunca senha |
| `md-d33.3.9` | Agent confetti | P3 | resumo animado ao encerrar sessão com ≥10 ações (ativo de marketing) |

Cada task tem descrição e critérios de aceite no beads (`bd show <id>`). Ordem sugerida:
guardrails → pair testing → workspaces → borda de foco → divisórias → launcher → tema →
badge → confetti.

**Já implementado nesta lista:** `md-d33.3.8` Guardrails (blocklist/redaction/auditoria),
`md-d33.3.5` Borda de foco por sessão (azul sem sessão, cor por agente, âmbar no lease;
validado por `scripts/focus_border_smoke.py`), `md-d33.3.7` Pair testing (botão ✋/▶ no
HUD, gate das tools do agente e tool `control_status`; validado por
`scripts/pair_testing_smoke.py`), `md-d33.3.11` PreviewSession (Unit of Work + Memento:
commit atômico, cancel por Esc, undo/redo) e `md-d33.3.2` divisórias arrastáveis
(percentuais `splits=` no config; validados por `scripts/layout_session_smoke.py`).

---

## T-005 — Auditoria visual: linha do tempo exportável (executar por último)

- **Status:** backlog · **Prioridade:** **P4 (última)** · **Task no beads:** `md-jcf.3`
- **Objetivo:** exportar “o que o agente viu/fez” (eventos MCP + frames/screenshots +
  timestamps) como linha do tempo para QA/auditoria.
- **Decisão de escopo:** **não reimplementar captura de tela** — o usuário já tem um
  software de gravação (nome a confirmar; “avifnim?” não reconhecido). A task deve
  **integrar**: exportar timeline JSON/CSV alinhada por timestamp (ou marcadores/capítulos
  no formato do gravador) e usar `capture_window` apenas como fallback de frames.
- **Inclui:** export JSON + HTML/Markdown navegável, filtro por sessão/agente, redaction de
  janelas sensíveis (depende de `md-d33.3.8`), retenção/limpeza, CLI
  `--audit export --session <id> --out <dir>`.
- **Aceite:** sessão exportada com eventos+frames, timeline legível, formato de
  sincronização documentado para o gravador externo, redaction, sem duplicar captura,
  docs e testes.
