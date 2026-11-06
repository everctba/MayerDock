; MayerDock — script do instalador (Inno Setup 6)
;
; Gera  dist\installer\MayerDock-Setup-<versão>.exe
;
; - O usuário escolhe a pasta na página "Selecionar pasta de destino" (DisableDirPage=no).
; - Padrão: instalação por usuário (sem admin) em {autopf}\MayerDock =
;   %LOCALAPPDATA%\Programs\MayerDock. O diálogo inicial permite escolher
;   "para todos os usuários" (aí {autopf} = Program Files, com UAC).
; - Fecha o MayerDock em execução antes de copiar (via FindWindow + WM_CLOSE).
; - Cria atalho no Menu Iniciar e (opcional) na área de trabalho.
; - Desinstalador padrão do Inno, com entrada em "Aplicativos instalados".

#define MyAppName "MayerDock"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "MayerDock"
#define MyAppExeName "mayerdock.exe"
#define MyAppId "{{7B4E3F0E-2C1A-4D6E-9B8F-5A1C2D3E4F50}"

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
; Página de escolha da pasta SEMPRE visível
DisableDirPage=no
DisableProgramGroupPage=yes
; instalação por usuário por padrão; o usuário pode escolher "todos"
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
OutputDir=..\dist\installer
OutputBaseFilename=MayerDock-Setup-{#MyAppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no
VersionInfoVersion={#MyAppVersion}
VersionInfoProductName={#MyAppName}
VersionInfoProductVersion={#MyAppVersion}
VersionInfoDescription=MayerDock - instalador
VersionInfoCompany={#MyAppPublisher}

[Languages]
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\dist\MayerDock\mayerdock.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\MayerDock\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\MayerDock\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\MayerDock\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
var
  CmdResult: Integer;

{ Fecha o MayerDock em execucao (taskkill sem /F envia WM_CLOSE, entao o app
  libera os slots e remove o AppBar antes de sair). }
procedure CloseMayerDock;
begin
  Exec(ExpandConstant('{cmd}'), '/c taskkill /IM mayerdock.exe', '', SW_HIDE,
    ewWaitUntilTerminated, CmdResult);
  Sleep(1500);
  { Se ainda estiver vivo (janela modal, travado), forca. }
  Exec(ExpandConstant('{cmd}'), '/c taskkill /IM mayerdock.exe /F', '', SW_HIDE,
    ewWaitUntilTerminated, CmdResult);
  Sleep(300);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  CloseMayerDock;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    CloseMayerDock;
end;
