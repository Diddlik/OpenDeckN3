; OpenDeckN3 – Windows-Installer (Inno Setup 6)
;
; Bauen (wird von .github/workflows/windows.yml aufgerufen):
;   cargo build --release -p n3-daemon
;   iscc /DAppVersion=0.1.0 installer\windows\opendeckn3.iss
;
; Installiert pro Benutzer (keine Admin-Rechte nötig) nach %LOCALAPPDATA%\Programs\OpenDeckN3.
; Profile/Einstellungen liegen in %APPDATA%\opendeckn3 und bleiben bei Deinstallation erhalten.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#define AppName "OpenDeckN3"
#define AppExe "opendeckn3d.exe"
#define Root "..\.."

[Setup]
AppId={{6B1E3F0A-4C2D-4E8B-9A57-0D3E5C7A9B21}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=OpenDeckN3
AppPublisherURL=https://github.com/Diddlik/OpenDeckN3
AppSupportURL=https://github.com/Diddlik/OpenDeckN3/issues
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
LicenseFile={#Root}\LICENSE
SetupIconFile={#Root}\assets\icon.ico
UninstallDisplayIcon={app}\icon.ico
OutputDir={#Root}\target\installer
OutputBaseFilename=OpenDeckN3-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "OpenDeckN3 bei der Anmeldung starten"; GroupDescription: "Start:"; Flags: unchecked

[Files]
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\assets\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion
; Beispiel-Plugin (läuft nur, wenn Node.js >= 22 installiert ist)
Source: "{#Root}\plugins\examples\*"; DestDir: "{app}\plugins"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; Parameters: "--open --plugins-dir ""{app}\plugins"""; WorkingDir: "{app}"; IconFilename: "{app}\icon.ico"
Name: "{group}\{#AppName} (ohne Hardware testen)"; Filename: "{app}\{#AppExe}"; Parameters: "--open --virtual --no-hardware --plugins-dir ""{app}\plugins"""; WorkingDir: "{app}"; IconFilename: "{app}\icon.ico"
Name: "{group}\Konfigurationsordner"; Filename: "{userappdata}\opendeckn3"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Parameters: "--open --plugins-dir ""{app}\plugins"""; WorkingDir: "{app}"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon
Name: "{userstartup}\{#AppName}"; Filename: "{app}\{#AppExe}"; Parameters: "--plugins-dir ""{app}\plugins"""; WorkingDir: "{app}"; IconFilename: "{app}\icon.ico"; Flags: runminimized; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Parameters: "--open --plugins-dir ""{app}\plugins"""; WorkingDir: "{app}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Dirs]
Name: "{userappdata}\opendeckn3"
