# MSIX / Microsoft Store — levantamento de requisitos e adaptações

**Data:** 2026-11-10 · **Status:** estudo (pré-implementação) · **Task:** [T-001](TASKS.md#t-001--build-msix-para-microsoft-store-em-paralelo-ao-instalador-atual)

Este documento lista o que o MayerDock precisa **mudar ou validar** para ser empacotado
como **MSIX** e publicado na Microsoft Store, mantendo o instalador Inno Setup e a pasta
portátil funcionando **em paralelo**.

---

## 1. Resumo executivo

O MayerDock é um bom candidato a MSIX: é um `.exe` único (425 KB), sem DLL de runtime
própria (só DLLs do sistema), sem assets obrigatórios em disco, sem rede, sem elevação,
DPI-aware v2 e sem serviços/drivers. **Não há bloqueio técnico estrutural.**

Mas há **3 adaptações obrigatórias** que hoje não existem (autostart saiu por decisão; e a
hipótese de redirecionamento de dados **não se confirmou** no teste real — ver §3/R4):

1. **Manifesto do pacote** com `runFullTrust` (capability restrita, precisa justificativa).
2. **Assets do pacote** (logos/tiles) e **versão em 4 partes** no `Identity`.
3. **Pipeline de build/assinatura** (`makeappx`/`signtool`/certificado; Store assina na
   publicação, sideload exige certificado confiável).

> **Decisão (2026-11-11):** **não haverá autostart** no MayerDock — nem `HKCU\Run` nem
> `startupTask`. O app é aberto pela bandeja/atalho/Menu Iniciar.

**Já implementado (F0, 2026-11-11):** app manifest Win32 embutido (`PerMonitorV2` +
`asInvoker`), re-registro de bandeja/AppBar em `TaskbarCreated`, detecção de modo pacote
(`GetCurrentPackageFullName`) e `--status` mostrando caminhos reais.

**Já implementado e VALIDADO (F1/F2, 2026-11-11):** manifesto/assets/pipeline MSIX;
pacote assinado, instalado por sideload e ativado (`empacotado=true`). Testes reais:
AppBar reservou a faixa (rcWork 1717), encaixe/liberação de janela externa funcionou,
bandeja criada, desinstalação limpa e **config preservado**. Detalhes em §5.

E há **4 itens que precisam de protótipo para validar** (AppBar e manipulação de janelas
sob identidade de pacote, bandeja, upgrade e convivência com a versão Inno).

---

## 2. O que já está pronto (não precisa mudar)

| Item | Situação |
| --- | --- |
| Binário único | `mayerdock.exe` 425 KB, sem dependência de DLL do MinGW (só `kernel32`, `user32`, `gdi32`, `shell32`, `msvcrt`, `ntdll`) — não precisa empacotar runtime |
| Assets em runtime | nenhum: ícone embutido no `.exe` (`assets/icon.ico`) |
| Rede / privacidade | nenhuma chamada de rede, nenhuma coleta |
| Elevação | nunca pede admin (perfeito: MSIX não eleva) |
| DPI | manifesto Win32 embutido (`PerMonitorV2`/`asInvoker`) + scaling lógico | totalmente compatível | nada a fazer |
| Instalação | não escreve na pasta do programa; só em `%APPDATA%` |
| Atualização | não tem auto-update próprio (Store pode gerenciar) |

---

## 3. O que muda no MSIX (impacto direto)

| Área | Hoje (Inno/portátil) | Com MSIX | Ação |
| --- | --- | --- | --- |
| **Pasta de instalação** | usuário escolhe (`DisableDirPage=no`) | `C:\Program Files\WindowsApps\...` — **fixa, somente leitura, sem escolha** | aceitar; a escolha de pasta deixa de existir no MSIX |
| **Desinstalação** | desinstalador Inno; dados preservados | pacote removido; **dados do usuário preservados** (validado) | nada a fazer |
| **Autostart** | não existe (decisão: sem autostart) | — | nada a fazer; recurso fora do produto |
| **Config/log** | `%APPDATA%\MayerDock\...` | **mesmo caminho real** (sem redirecionamento em full trust — validado) | `--status` continua mostrando o caminho real |
| **Identidade** | sem identidade | app ganha package identity (SingleInstance, AppData, etc.) | adaptar mensagens e single-instance |
| **Atualização** | manual (Inno) | Store atualiza e **fecha o app** | testar upgrade com app aberto |
| **Ícones/tiles** | ícone embutido no `.exe` | manifesto exige PNGs de logo | gerar assets a partir de `assets/icon.png` |
| **Assinatura** | nenhuma (Inno não assina por padrão) | Store assina na publicação; sideload exige cert | pipeline de assinatura p/ teste local |
| **Convivência** | só uma versão | Inno + MSIX podem coexistir (duas AppBars, dois trays, mesma classe) | detectar e avisar/bloquear execução simultânea |

---

## 4. Requisitos obrigatórios (checklist técnico)

### R1. Manifesto do pacote (`Package.appxmanifest`)
- `<Application Id="MayerDock" Executable="mayerdock.exe"
   EntryPoint="Windows.FullTrustApplication">` com `uap10:RuntimeBehavior="packagedClassicApp"`
  e `uap10:TrustLevel="mediumIL"` (esquema `uap10`), **ou** o par equivalente aceito pelo
  MSIX Packaging Tool.
- `<rescap:Capability Name="runFullTrust"/>` — **restricted capability**: a Store exige
  justificativa na submissão (texto sugerido no §7).
- `TargetDeviceFamily`: `Windows.Desktop`, `MinVersion=10.0.17763.0`,
  `MaxVersionTested=10.0.22621.0` (ou superior).
- `Identity Version` em **4 partes** (`0.1.0.0`); precisa ser **maior** a cada novo envio.
- `ProcessorArchitecture="x64"`.

### R2. Assets do pacote (PNG)
Mínimo para `uap:VisualElements` + `Properties`:
- `Square150x150Logo.png` (150×150), `Square44x44Logo.png` (44×44), `StoreLogo.png` (50×50).
Recomendado gerar variantes de escala (`scale-100/125/150/200/400` e `targetsize-*` para
44) a partir de `assets/icon.png` (512×512) com Pillow.

### R3. Autostart — **fora de escopo (decidido)**
- **Decisão:** o MayerDock **não terá** "iniciar com o Windows". Sem `startupTask` no MSIX
  e sem `HKCU\Run` na versão Inno/portátil.
- Se um dia voltar: `desktop6:Extension Category="windows.startupTask"` + WinRT
  `Windows.ApplicationModel.StartupTask` (crate `windows`, não `windows-sys`).

### R4. Dados e virtualização — **validado: sem redirecionamento**
- Teste real (sideload, Windows 10 19041): o app **full trust** gravou em
  `%APPDATA%\MayerDock\` real — `config.ini` e `mayerdock.log` **não** foram para
  `LocalCache`. Portanto:
  - o `--status` mostra o caminho real e ele é o mesmo da versão Inno;
  - **desinstalar o MSIX não apaga a configuração** (verificado);
  - não é necessário Exportar/Importar config para migrar entre edições.
- Observação: isso vale para pacote **full trust (`runFullTrust`)**; apps em app container
  (UWP) continuam com redirecionamento. Revalidar em outras versões do Windows.
- Detecção de modo pacote já implementada (`GetCurrentPackageFullName` + `--status`), útil
  para mensagens e suporte mesmo sem diferença de caminho.

### R5. Assinatura
- **Sideload/local:** MSIX precisa de assinatura confiável. O `build-msix.ps1 -SelfSign`
  cria o certificado `CN=MayerDock Dev` em `CurrentUser\My`, exporta `.cer`, confia em
  `CurrentUser\TrustedPeople` e assina com `signtool` — **feito**. Atenção: instalar exige
  a **raiz** confiável; `CurrentUser\TrustedPeople` sozinho dá `0x800B0109`. Para sideload
  nesta máquina, uma vez **como admin**:
  `certutil -addstore TrustedPeople dist\msix\mayerdock-dev.cer` (ou LocalMachine\TrustedPeople);
  alternativa: Modo de Desenvolvedor.
- **Store:** subir o pacote **sem** assinatura de distribuição; a Store assina.
- `signtool.exe` faz parte do Windows SDK / SDK BuildTools (`.tools\`).

### R6. Ferramentas de build
Não há Windows SDK instalado nesta máquina (verificado: `makeappx`, `signtool`, `msixmgr`
ausentes). Solução adotada (sem admin): baixar o pacote NuGet
**`Microsoft.Windows.SDK.BuildTools` 10.0.22621.3233** e extrair em `.tools\` (ignorado
pelo git). O `build-msix.ps1` procura `makeappx.exe`/`signtool.exe` no Windows SDK, no
PATH, em `MAKEAPPX`/`SIGNTOOL` ou em `.tools\`.

> A versão **10.0.28000.x** do mesmo pacote **não executa** no Windows 10 19041
> (`not a valid application`) — usar a linha **22621**.
Outras opções: Windows SDK completo, MSIX Packaging Tool, ou CI (`windows-latest` já tem
SDK).

### R7. Pipeline paralelo (não quebrar o atual)
- Nova flag isolada `build.ps1 -Msix` → `dist\msix\MayerDock-0.1.0.msix`.
- `-Installer` (Inno), `-Run` e `-Shortcut` **intocados**.
- Nunca gerar MSIX a partir de `dist\MayerDock` "sujo"; usar a mesma `dist\MayerDock` como
  fonte de arquivos.

### R8. App manifest Win32 embutido — **FEITO (2026-11-11)**
O `.exe` agora embute `assets/app.manifest` (`build.rs` → recurso `RT_MANIFEST`) com
`PerMonitorV2`, `asInvoker`, `longPathAware` e `supportedOS`. Verificado no binário
(`PerMonitorV2`, `asInvoker`, `<assembly` presentes).

### R9. Bandeja e Explorer reiniciando — **FEITO (2026-11-11)**
`RegisterWindowMessageW("TaskbarCreated")` + handler: remove e re-adiciona o ícone
(idempotente), re-registra a AppBar (`ABM_REMOVE` + `ABM_NEW` + `ABM_SETPOS`), restaura
topmost, re-sincroniza as janelas e reergue a camada do FluxTV. Sem polling.

### R10. Execução simultânea das duas edições
MSIX e Inno compartilham classe de janela (`MayerDock.DockWindow.v1`) e bandeja.
Com as duas abertas: dois AppBars, duas faixas reservadas. Ação: detectar outra instância
inclusive de "edição" diferente e avisar/sair.

### R11. CLI / execution alias (opcional)
`--status`, `--version` e `--help` já existem na versão portátil/Inno **e** no MSIX (o
`--status` informa `packaged: true` + caminhos reais). Para chamar `mayerdock.exe --status`
de qualquer terminal com o pacote instalado, adicionar:
`uap5:Extension Category="windows.appExecutionAlias"` com `Alias="mayerdock.exe"`.

---

## 5. Validação (F2) — resultados reais

Ambiente: Windows 10 19041 · sideload habilitado (Developer Mode + certificado de dev em
`LocalMachine\TrustedPeople`) · pacote `MayerDock.Dev 0.1.0.0 x64` (0,32 MB) assinado com
certificado de dev. Instalado via `Add-AppxPackage` e ativado por `shell:AppsFolder\...`.

| # | Item | Resultado |
| --- | --- | --- |
| V1 | `SHAppBarMessage` sob MSIX | ✅ reserva real: `rcWork.right = 1717` (2560 − taskbar − dock 772) |
| V2 | `SetWindowPos` em janela de outro processo | ✅ janela encaixada na divisão 0 e liberada |
| V3 | `Shell_NotifyIcon` com identity | ✅ "ícone da bandeja criado (embutido)" |
| V4 | `SetWinEventHook` / `RegisterHotKey` | ✅ Start/pesquisa com `fg=SearchApp` → dock oculta (rcWork 2489) e volta (1717); `Ctrl+Alt+M` oculta/mostra; hook e hotkey funcionam no pacote |
| V5 | `SHQueryUserNotificationState` (fullscreen) | ⏳ não exercitado com fullscreen real (jogo/vídeo); teste com janela borderless não virou foreground |
| V6 | Upgrade MSIX 0.1.0 → 0.1.1 | ✅ app fechado: upgrade aplicado (`0.1.1.0`) e **config preservado**; app aberto: `0x80073D02` ("recursos em uso") — esperado no sideload; a Store gerencia/fecha o app |
| V7 | Uninstall + dados | ✅ pacote removido; **`config.ini` preservado** |
| V8 | Inno + MSIX simultâneos | ✅ **já coberto pela instância única**: com o pacote rodando, abrir o portátil/log `instância já em execução` e não sobe segundo dock (1 processo) |
| V9 | Executar `.exe` direto de `WindowsApps` | ❌ `Access is denied` (esperado; ativar pelo shell/alias) |

Observações:
- `empacotado=true` no log e `--status` detecta o modo pacote corretamente.
- `--status` via terminal exige **execution alias** (R11) — rodar o `.exe` da `WindowsApps`
  diretamente não funciona.
- Sem redirecionamento de `%APPDATA%` (ver R4).

Se V1 falhasse (não foi o caso), fallback: reserva via `SystemParametersInfo` (aproximada)
ou distribuir só como EXE/MSI.

---

## 6. Requisitos de Store (fora do código)

- **Reservar o nome** no Partner Center (identidade `Name`/`Publisher` passam a ser da loja;
  o build de sideload usa identidade própria — manter manifest **parametrizado**).
- **Justificativa de `runFullTrust`**: "o app precisa integrar com o shell (AppBar/dock) e
  gerenciar janelas do usuário para encaixá-las; não eleva privilégios, não coleta dados".
- **WACK** (Windows App Certification Kit) aprovado.
- **Listagem**: descrição, screenshots (1366×768+), ícone de loja, classificação etária,
  contato de suporte.
- **Política de privacidade**: não há coleta; declarar "não se aplica" (a Store pode pedir
  URL mesmo assim — verificar na submissão).
- **Preço/payout**: definir modelo (pago, gratuito, trial via add-on). O código não muda.
- **Manutenção**: cada envio exige versão nova (4 partes) e pode exigir re-certificação.

---

## 7. Insights (os que mais importam)

1. **MSIX não deixa escolher a pasta de instalação.** Se "escolher onde instalar" é
   requisito de produto, ele continua sendo atendido **só pelo instalador Inno**; o MSIX
   instala em `WindowsApps`. Isso é uma decisão de produto, não um bug.
2. **"Iniciar com o Windows" — decidido: não existe.** O app não tem autostart em nenhuma
   edição; nada a implementar (se voltar, usar `startupTask`, nunca `HKCU\Run`).
3. **Dados do usuário: sem redirecionamento em full trust (validado).** `%APPDATA%` real é
   usado e a config **sobrevive** ao uninstall — não é preciso Exportar/Importar para
   migrar edições (só para backup, se quiser).
4. **Não perseguir `unvirtualizedResources`**: é capability restrita, raramente aprovada
   para apps de produtividade; mantenha os dados por pacote.
5. **Detecção de modo empacotado — feita.** `--status` mostra `packaged`/`package` e os
   caminhos reais; é o que permite suportar MSIX sem "mentir" para o usuário.
6. **Manifesto Win32 — feito** (`PerMonitorV2`/`asInvoker`): DPI correto no primeiro frame
   e melhor resultado no WACK.
7. **`TaskbarCreated` — feito**: bandeja e AppBar voltam sozinhas após restart do Explorer
   (qualidade de base, vale para as duas edições).
8. **Rodar Inno e MSIX juntos conflita** (duas AppBars/trays). Detectar a outra instância e
   avisar é simples e evita suporte confuso.
9. **A Store aceita EXE/MSI (Win32) além de MSIX.** Se a escolha de pasta for inegociável,
   vale investigar a submissão do **instalador Inno** como EXE/MSI (com limitações de
   atualização/silent install) em vez de forçar o MSIX — decisão a validar com a política
   atual da Store.
10. **Identidade da Store só existe após reservar o nome.** Até lá, manter
    `-IdentityName`/`-Publisher` como parâmetros do script (placeholders), sem commitar a
    identidade real no repositório.
11. **Versão do pacote ≠ versão do Cargo** por formato: derivar `0.1.0` → `0.1.0.0` no
    script; nunca reutilizar a mesma versão na Store.
12. **Tamanho/dependências já estão ok**: 425 KB, sem DLL extra — o pacote MSIX deve ficar
    na casa de ~1 MB comprimido.

---

## 8. Plano sugerido (fases e esforço)

| Fase | Entrega | Esforço |
| --- | --- | --- |
| F0 | App manifest Win32 (`PerMonitorV2`/`asInvoker`) + `TaskbarCreated` + detecção de pacote (`GetCurrentPackageFullName`) + `--status` | **FEITO** (2026-11-11) |
| F1 | `Package.appxmanifest` + assets + `build-msix.ps1` + `build.ps1 -Msix` + assinatura de teste | **FEITO** (build/assinatura/validação); instalação local pendente de admin |
| F2 | Validação V1–V9 em sideload | **em andamento**: V1/V2/V3/V6/V7 ✅; V4/V5/V8 pendentes |
| F3 | Export/Import de config + mensagens de modo pacote | 1 dia |
| F4 | WACK + listagem + submissão à Store (reserva de nome, screenshots, justificativa) | 1 dia + review |

**Total realista:** ~1 semana de trabalho + tempo de review da Store.

---

## 9. Critérios de aceite (MSIX)

- [ ] `.\build.ps1 -Msix` gera `dist\msix\MayerDock-0.1.0.msix` **sem** alterar
      `dist\installer\` nem `dist\MayerDock\`.
- [ ] `.\build.ps1 -Installer` continua idêntico (regressão zero).
- [ ] Pacote assinado instala via `Add-AppxPackage` (sideload) e abre.
- [ ] AppBar reserva a faixa e janelas encaixam (V1/V2).
- [ ] Bandeja aparece com ícone próprio; menu/botões funcionam (V3).
- [ ] Start/pesquisa e fullscreen continuam disparando o comportamento correto (V4/V5).
- [ ] Upgrade 0.1.0 → 0.1.1 preserva config; desinstalar remove pacote (dados: documentado).
- [ ] Inno + MSIX não rodam juntos (aviso) (V8).
- [ ] WACK aprovado; `--status`/log informam o caminho real no modo pacote.

---

## 10. Referências

- MSIX — empacotar app desktop: `learn.microsoft.com/windows/msix/desktop/desktop-to-uwp`
- `runFullTrust` (restricted capability) e justificativa: `.../msix/package/restricted-capabilities`
- `desktop6:startupTask`: `.../windows/msix/desktop/desktop-to-uwp-extensions#startup-task`
- App execution alias (`uap5`): `.../windows/msix/desktop/desktop-to-uwp-extensions`
- WACK: `.../windows/win32/appcertificationkit/using-the-windows-app-certification-kit`
- Store: pacotes MSIX vs EXE/MSI (`.../windows/apps/publish/`), Partner Center.
