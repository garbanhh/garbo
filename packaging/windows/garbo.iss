; Instalador de Garbo para Windows (Inno Setup 6). Lo genera scripts/instalador.ps1, que pasa
; /DVersion=X.Y.Z y /DFuente=<carpeta con garbo.exe, conpty.dll y OpenConsole.exe>.
; Se instala para el usuario, sin permisos de administrador, en %LOCALAPPDATA%\Programs\Garbo.

#ifndef Version
  #error Falta /DVersion=X.Y.Z
#endif
#ifndef Fuente
  #define Fuente "..\..\target\release"
#endif

[Setup]
; Identificador fijo: las versiones nuevas se instalan encima de la anterior.
AppId={{6F2C1B8E-3A4D-4C7B-9E21-6A0D5B8C2F17}
AppName=Garbo
AppVersion={#Version}
AppVerName=Garbo {#Version}
AppPublisher=garbanhh
AppPublisherURL=https://github.com/garbanhh/garbo
AppSupportURL=https://github.com/garbanhh/garbo/issues
AppUpdatesURL=https://github.com/garbanhh/garbo/releases
DefaultDirName={autopf}\Garbo
DefaultGroupName=Garbo
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
LicenseFile=..\..\LICENSE
SetupIconFile=..\io.github.garbanhh.Garbo.ico
UninstallDisplayIcon={app}\garbo.exe
UninstallDisplayName=Garbo
OutputDir=..\..\target\windows
OutputBaseFilename=garbo-{#Version}-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Si Garbo está abierto se pide cerrarlo (y se vuelve a abrir al terminar).
CloseApplications=yes
RestartApplications=yes

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Fuente}\garbo.exe"; DestDir: "{app}"; Flags: ignoreversion
; ConPTY de Windows Terminal (MIT): secuencias en orden y salida mucho más rápida.
Source: "{#Fuente}\conpty.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Fuente}\OpenConsole.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "..\..\NOTICE"; DestDir: "{app}"; DestName: "NOTICE.txt"

[Icons]
; Con el mismo identificador que fija Garbo al arrancar: sus ventanas y sus avisos se agrupan bajo
; este acceso directo.
Name: "{autoprograms}\Garbo"; Filename: "{app}\garbo.exe"; AppUserModelID: "io.github.garbanhh.Garbo"
Name: "{autodesktop}\Garbo"; Filename: "{app}\garbo.exe"; AppUserModelID: "io.github.garbanhh.Garbo"; Tasks: desktopicon

[Registry]
; Nombre e icono de los avisos (Garbo los escribe al primer aviso): se borran al desinstalar.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\io.github.garbanhh.Garbo"; Flags: uninsdeletekey

[Run]
Filename: "{app}\garbo.exe"; Description: "{cm:LaunchProgram,Garbo}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Caché (scripts de integración, capturas pegadas, icono de los avisos). La configuración
; (%APPDATA%\garbo) y el historial se conservan.
Type: filesandordirs; Name: "{localappdata}\garbo"
