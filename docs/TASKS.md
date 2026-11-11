# Backlog / tarefas — MayerDock

Tarefas futuras que **não** devem quebrar o que já funciona. O instalador atual
(Inno Setup, `build.ps1 -Installer`) é o caminho oficial de distribuição até a Store
estar pronta.

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
- [ ] V4/V5 (hook de foreground/hotkey/fullscreen) e V8 (Inno+MSIX simultâneos) — pendentes.
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
