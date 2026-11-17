; Instalador de Rust Sonar (Inno Setup 6).
;
; Compilar: powershell -ExecutionPolicy Bypass -File installer\build.ps1
;
; Instala la app (Archivos de programa, menú Inicio, desinstalador, inicio con Windows) y, si faltan,
; los drivers de cables virtuales de VB-Audio: VB-CABLE (micrófono virtual), Hi-Fi Cable (canal
; Juego) y Voicemeeter (canales Chat, Media y Aux).
;
; Los drivers NO van dentro del instalador: la licencia de VB-Audio no permite integrarlos en el de
; otro programa. Se descargan durante la instalación de download.vb-audio.com (sus paquetes oficiales,
; tal cual) y se comprueba su SHA-256. Así el instalador se puede distribuir.

#ifndef AppVersion
  #define AppVersion "0.2.0"
#endif
#define AppName "Rust Sonar"
#define AppExe "rust_sonar.exe"
#define VbAudio "https://download.vb-audio.com/Download_CABLE/"

[Setup]
AppId={{6E0B5A43-2C1D-4C59-9C1E-5B7A3F0D8E21}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=kilorito
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Los drivers necesitan administrador.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
OutputDir=..\dist
OutputBaseFilename=RustSonar-Setup-{#AppVersion}
SetupIconFile=..\assets\rust_sonar.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
VersionInfoCompany=kilorito
VersionInfoDescription=Instalador de {#AppName}
#ifdef Sign
; Firma setup y desinstalador con el certificado "kilorito" (orden definida por build.ps1 con /Skilorito=...).
SignTool=kilorito
SignedUninstaller=yes
#endif
; Tema oscuro con los colores de Rust Sonar (también lo usa el desinstalador).
WizardStyle=modern dark hidebevels includetitlebar
WizardBackColor=#0d0f13
WizardImageFile=img\wizard-100.bmp,img\wizard-150.bmp,img\wizard-200.bmp
WizardSmallImageFile=img\wizard-small-55.bmp,img\wizard-small-83.bmp,img\wizard-small-110.bmp
Compression=lzma2/max
SolidCompression=yes
; Para descomprimir los .zip de VB-Audio descargados ("enhanced" solo abre .7z).
ArchiveExtraction=full
InfoBeforeFile=LEEME-instalador.txt
; El inicio automático va en HKCU del usuario que instala (el mismo que acepta el aviso de administrador).
UsedUserAreasWarning=no
; Rust Sonar se cierra a mano en PrepareToInstall (a través de su API, para que deje Windows en orden).
CloseApplications=no

[Languages]
; El instalador propio (installer/ui) elige el idioma con /LANG=en|es según el de Windows.
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"

[CustomMessages]
en.GroupDevices=Virtual devices:
es.GroupDevices=Dispositivos virtuales:
en.GroupOptions=Options:
es.GroupOptions=Opciones:
en.TaskDrivers=Install the missing VB-Audio virtual cables (VB-CABLE, Hi-Fi Cable and Voicemeeter)
es.TaskDrivers=Instalar los cables virtuales de VB-Audio que falten (VB-CABLE, Hi-Fi Cable y Voicemeeter)
en.TaskAutostart=Open Rust Sonar when Windows starts (in the tray)
es.TaskAutostart=Abrir Rust Sonar al iniciar Windows (en la bandeja)
en.StatusCable=Installing VB-CABLE (virtual microphone)... If Windows asks about the driver, press Install.
es.StatusCable=Instalando VB-CABLE (micrófono virtual)... Si Windows pregunta por el driver, pulsa Instalar.
en.StatusHifi=Installing Hi-Fi Cable (Game channel)... If Windows asks about the driver, press Install.
es.StatusHifi=Instalando Hi-Fi Cable (canal Juego)... Si Windows pregunta por el driver, pulsa Instalar.
en.StatusVm=Installing Voicemeeter (Chat, Media and Aux channels): press "Install" in its window, then OK.
es.StatusVm=Instalando Voicemeeter (canales Chat, Media y Aux): pulsa «Install» en su ventana y después OK.
en.StatusTidy=Setting up the audio devices (Windows will only show Rust Sonar's)...
es.StatusTidy=Configurando los dispositivos de audio (en Windows solo quedarán los de Rust Sonar)...

[Tasks]
Name: "drivers"; Description: "{cm:TaskDrivers}"; GroupDescription: "{cm:GroupDevices}"
Name: "autostart"; Description: "{cm:TaskAutostart}"; GroupDescription: "{cm:GroupOptions}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:GroupOptions}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
; Paquetes oficiales de VB-Audio: se descargan (solo los que falten), se comprueba su SHA-256, se
; descomprimen en la carpeta temporal y se borran al terminar.
Source: "{#VbAudio}VBCABLE_Driver_Pack46.zip"; DestName: "VBCABLE_Driver_Pack46.zip"; DestDir: "{tmp}\drivers\vbcable"; ExternalSize: 1324680; Hash: "bc44e8974191a5a2427d6c979e2dab08dbb09925f3f435dfa2f0ba3e45b6eb90"; Flags: external download extractarchive recursesubdirs createallsubdirs ignoreversion deleteafterinstall; Tasks: drivers; Check: DriverMissing('VBAudioVACMME')
Source: "{#VbAudio}HiFiCableAsioBridgeSetup_v1007.zip"; DestName: "HiFiCableAsioBridgeSetup_v1007.zip"; DestDir: "{tmp}\drivers\hificable"; ExternalSize: 4006986; Hash: "3ecf204bfd8579d36bb918f9856eb1eaddd49c75146f5e8a8f59dcb8375ae89a"; Flags: external download extractarchive recursesubdirs createallsubdirs ignoreversion deleteafterinstall; Tasks: drivers; Check: DriverMissing('VBAudioHFVAIOMME')
Source: "{#VbAudio}Voicemeeter8Setup_v3130.zip"; DestName: "Voicemeeter8Setup_v3130.zip"; DestDir: "{tmp}\drivers\voicemeeter"; ExternalSize: 73948387; Hash: "f0772b275bb6fe0d4b2a5a2fa4d25d1b5a6ee0a8ee02ba79574c9cdd85fb6fa3"; Flags: external download extractarchive recursesubdirs createallsubdirs ignoreversion deleteafterinstall; Tasks: drivers; Check: DriverMissing('VBVoicemeeterVAIOMME')

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Mismo valor que escribe la propia app al activar "Iniciar con Windows" (system.rs).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "RustSonar"; ValueData: """{app}\{#AppExe}"" --minimized"; Flags: uninsdeletevalue; Tasks: autostart

[Run]
; -i -h = instalación silenciosa de los instaladores de VB-Audio. Windows puede preguntar igualmente
; si confías en el driver: hay que aceptar. Voicemeeter no tiene modo silencioso: se abre su ventana.
Filename: "{tmp}\drivers\vbcable\VBCABLE_Setup_x64.exe"; Parameters: "-i -h"; WorkingDir: "{tmp}\drivers\vbcable"; StatusMsg: "{cm:StatusCable}"; Flags: waituntilterminated; Tasks: drivers; Check: DriverMissing('VBAudioVACMME'); AfterInstall: DriverInstalled
Filename: "{tmp}\drivers\hificable\HiFiCableAsioBridgeSetup.exe"; Parameters: "-i -h"; WorkingDir: "{tmp}\drivers\hificable"; StatusMsg: "{cm:StatusHifi}"; Flags: waituntilterminated; Tasks: drivers; Check: DriverMissing('VBAudioHFVAIOMME'); AfterInstall: DriverInstalled
Filename: "{tmp}\drivers\voicemeeter\Voicemeeter8Setup.exe"; WorkingDir: "{tmp}\drivers\voicemeeter"; StatusMsg: "{cm:StatusVm}"; Flags: waituntilterminated; Tasks: drivers; Check: DriverMissing('VBVoicemeeterVAIOMME'); AfterInstall: DriverInstalled
; Con los cables ya activos (sin drivers nuevos), se configura y ordena aquí mismo, aprovechando que el
; instalador ya es administrador: Windows se queda solo con «Rust Sonar - Juego», «- Chat»… y sin
; preguntar nada. Con drivers recién instalados hay que reiniciar antes; lo hará la app en su primer
; arranque (con un aviso de administrador).
Filename: "{app}\{#AppExe}"; Parameters: "--setup-devices"; StatusMsg: "{cm:StatusTidy}"; Flags: waituntilterminated runhidden; Check: not NeedRestart
; Sin drivers nuevos no hace falta reiniciar: se ofrece abrir la app (como el usuario, no como administrador).
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent runasoriginaluser; Check: not NeedRestart

[UninstallRun]
; Antes de borrar los archivos: dispositivos visibles con su nombre, predeterminados en los reales,
; sin inicio automático. Los drivers de VB-Audio se quedan (se quitan desde "Aplicaciones" de Windows).
Filename: "{app}\{#AppExe}"; Parameters: "--uninstall-cleanup"; Flags: waituntilterminated runhidden; RunOnceId: "RustSonarCleanup"

[Code]
var
  DriversInstalled: Boolean;

{ ¿Falta el driver? Se mira su servicio de Windows. }
function DriverMissing(Service: String): Boolean;
begin
  Result := not RegKeyExists(HKLM, 'SYSTEM\CurrentControlSet\Services\' + Service);
end;

procedure DriverInstalled();
begin
  DriversInstalled := True;
end;

{ Los drivers nuevos no funcionan bien hasta reiniciar. }
function NeedRestart(): Boolean;
begin
  Result := DriversInstalled;
end;

{ Cierra Rust Sonar si está abierto: primero por su API (así devuelve los predeterminados de
  Windows y deshace el enrutado de las apps) y, si no responde, a la fuerza. }
procedure CloseRustSonar();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -Command "try { Invoke-WebRequest -UseBasicParsing -Method Post http://127.0.0.1:7878/api/quit -TimeoutSec 3 | Out-Null; Start-Sleep -Seconds 3 } catch {}"',
    '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseRustSonar();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  CloseRustSonar();
  Result := True;
end;
