; The maintenance helper is the sole owner of payloads, COM, MSIX and PATH.
; Inno owns its uninstall log, ARP entry and shortcut only.
#ifndef PayloadDir
  #error PayloadDir must name the verified staged payload
#endif
#ifndef AppVersion
  #error AppVersion is required
#endif
#include "installer-messages.iss"

[Setup]
AppId={{{#AppId}}
AppName={#InstallName}
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
VersionInfoProductVersion={#AppVersion}
AppVerName={#InstallName} {#AppVersion}
DefaultDirName={localappdata}\Programs\{#InstallName}
DisableDirPage=yes
UsePreviousAppDir=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0
CloseApplications=no
RestartApplications=no
ChangesEnvironment=yes
UninstallDisplayIcon={app}\versions\{#AppVersion}\unlinka-desktop.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
OutputDir={#OutputDir}
OutputBaseFilename=Unlinka-Setup-{#AppVersion}-x64
SetupIconFile=Assets\Unlinka.ico
Uninstallable=yes
#if SignedBuild == "1"
SignTool=unlinka_signed_artifact
SignedUninstaller=yes
#endif

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "zh"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Tasks]
Name: "menus"; Description: "{cm:Menus}"
Name: "path"; Description: "{cm:UserPath}"

[Files]
Source: "{#MaintenanceExe}"; DestDir: "{tmp}\tools"; Flags: dontcopy noencryption
Source: "{#LauncherExe}"; DestDir: "{tmp}\tools"; Flags: dontcopy noencryption
Source: "{#PayloadDir}\*"; DestDir: "{tmp}\payload"; Flags: dontcopy noencryption recursesubdirs

[Icons]
Name: "{userprograms}\{#InstallName}"; Filename: "{app}\versions\{#AppVersion}\unlinka-desktop.exe"; Parameters: "--settings"; WorkingDir: "{app}"

[UninstallDelete]
Type: dirifempty; Name: "{app}"

[Code]
var
  LanguagePage: TInputOptionWizardPage;
  Installed: Boolean;
  UninstallHelper: String;

function BoolText(Value: Boolean): String;
begin
  if Value then Result := 'true' else Result := 'false';
end;

procedure InitializeWizard;
var
  LanguageValue, SelectedTasks: String;
  MenusValue, PathValue: Cardinal;
begin
  LanguagePage := CreateInputOptionPage(wpSelectTasks, CustomMessage('LanguageTitle'),
    CustomMessage('LanguagePrompt'), '', True, False);
  LanguagePage.Add(CustomMessage('LanguageAuto'));
  LanguagePage.Add('简体中文');
  LanguagePage.Add('English');
  LanguagePage.SelectedValueIndex := 0;
  if RegQueryStringValue(HKCU64, '{#SettingsKey}', 'Language', LanguageValue) then begin
    if LanguageValue = 'zh-CN' then LanguagePage.SelectedValueIndex := 1;
    if LanguageValue = 'en-US' then LanguagePage.SelectedValueIndex := 2;
  end;
  if not RegQueryDWordValue(HKCU64, '{#SettingsKey}', 'MenusEnabled', MenusValue) then MenusValue := 1;
  if not RegQueryDWordValue(HKCU64, '{#SettingsKey}', 'PathEnabled', PathValue) then PathValue := 1;
  SelectedTasks := '';
  if MenusValue <> 0 then SelectedTasks := 'menus';
  if PathValue <> 0 then begin
    if SelectedTasks <> '' then SelectedTasks := SelectedTasks + ',';
    SelectedTasks := SelectedTasks + 'path';
  end;
  WizardSelectTasks(SelectedTasks);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Parameters, LanguageValue: String;
  ResultCode: Integer;
begin
  Result := '';
  if Installed then Exit;
  ExtractTemporaryFiles('{tmp}\tools\*');
  ExtractTemporaryFiles('{tmp}\payload\*');
  LanguageValue := 'auto';
  if LanguagePage.SelectedValueIndex = 1 then LanguageValue := 'zh-CN';
  if LanguagePage.SelectedValueIndex = 2 then LanguageValue := 'en-US';
  Parameters := 'install --source "' + ExpandConstant('{tmp}\payload') +
    '" --launcher "' + ExpandConstant('{tmp}\tools\unlinka-launcher.exe') +
    '" --maintenance "' + ExpandConstant('{tmp}\tools\unlinka-maintenance.exe') +
    '" --language ' + LanguageValue + ' --menus ' + BoolText(WizardIsTaskSelected('menus')) +
    ' --path ' + BoolText(WizardIsTaskSelected('path'));
  if not Exec(ExpandConstant('{tmp}\tools\unlinka-maintenance.exe'), Parameters,
    ExpandConstant('{tmp}'), SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Result := CustomMessage('MaintenanceFailed')
  else if ResultCode <> 0 then
    Result := CustomMessage('MaintenanceFailed') + ' (' + IntToStr(ResultCode) + ')'
  else Installed := True;
end;

function InitializeUninstall: Boolean;
begin
  { InitializeUninstall runs before Inno's confirmation. Do not mutate the
    installation until usAppMutexCheck, which runs after that confirmation. }
  Result := False;
  if not FileExists(ExpandConstant('{app}\maintenance\unlinka-maintenance.exe')) then begin
    MsgBox(CustomMessage('RepairFirst'), mbError, MB_OK);
    Exit;
  end;
  { Waiting on the installed EXE would keep it mapped and make its own deletion
    impossible. Inno executes a temporary copy and waits for the final outcome. }
  UninstallHelper := ExpandConstant('{tmp}\unlinka-uninstall-maintenance.exe');
  if not CopyFile(ExpandConstant('{app}\maintenance\unlinka-maintenance.exe'), UninstallHelper, False) then begin
    MsgBox(CustomMessage('MaintenanceFailed'), mbError, MB_OK);
    Exit;
  end;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ResultCode: Integer;
begin
  if CurUninstallStep <> usAppMutexCheck then Exit;
  { Abort exits silently before Inno removes its ARP, log, or shortcut. The
    helper adds its own confirmation with the option to purge preferences. }
  if not Exec(UninstallHelper, 'uninstall', ExpandConstant('{localappdata}'),
    SW_HIDE, ewWaitUntilTerminated, ResultCode) then begin
    MsgBox(CustomMessage('MaintenanceFailed'), mbError, MB_OK);
    Abort;
  end;
  if ResultCode = 5 then Abort;
  if ResultCode = 4 then begin
    MsgBox(CustomMessage('RemovalPending'), mbInformation, MB_OK);
    Abort;
  end;
  if ResultCode <> 0 then begin
    MsgBox(CustomMessage('MaintenanceFailed') + ' (' + IntToStr(ResultCode) + ')', mbError, MB_OK);
    Abort;
  end;
end;
