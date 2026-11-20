#ifndef AppVersion
  #define AppVersion "0.0.0.1"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

#define AppName "CustomDock"
#define AppExe "custom-dock.exe"

[Setup]
AppId={{5C1E2B7A-7D44-4B8E-9C61-2F0D7A3E9B15}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=CustomDock (based on CustomDock)
AppPublisherURL=https://github.com/FalahRauf/custom-dock
AppSupportURL=https://github.com/FalahRauf/custom-dock/issues
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
LicenseFile=..\LICENSE.md
OutputDir=..\dist
OutputBaseFilename=CustomDock-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile=..\assets\abra.ico
CloseApplications=no

[Tasks]
Name: "autostart"; Description: "Launch CustomDock when I sign in to Windows"; Flags: unchecked
Name: "startmenu"; Description: "Create a Start menu shortcut"

[Files]
Source: "{#SourceDir}\custom-dock.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\custom-dock-guard.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\fonts\*"; DestDir: "{app}\fonts"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\docs\PANDUAN.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: startmenu

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "CustomDock"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch CustomDock now"; Flags: nowait postinstall skipifsilent

[Code]
procedure StopApp();
var
  rc: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM custom-dock.exe', '', SW_HIDE, ewWaitUntilTerminated, rc);
  Sleep(2500);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM custom-dock-guard.exe', '', SW_HIDE, ewWaitUntilTerminated, rc);
  Sleep(300);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopApp();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  StopApp();
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    if MsgBox('Also delete your CustomDock settings (%APPDATA%\CustomDock)?', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{userappdata}\CustomDock'), True, True, True);
  end;
end;
