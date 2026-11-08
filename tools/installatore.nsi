; Installatore di openFatt per Windows (NSIS 3). Si crea con tools\crea-installatore.ps1.
;
; Si installa per il solo utente, senza diritti di amministratore. Eseguito sopra una versione
; precedente la sostituisce: archivi e preferenze stanno in Documenti\openFatt e in
; %APPDATA%\openFatt, fuori dalla cartella del programma, e non vengono toccati.

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user

!include "MUI2.nsh"
!include "FileFunc.nsh"

; /RIAVVIA: aggiornamento partito dal programma, che attende la sua chiusura e poi lo riapre.
; ISOLATO: solo per le prove, non tocca registro di sistema e collegamenti.
Var Riavvia
Var Attese

!define CHIAVE "Software\Microsoft\Windows\CurrentVersion\Uninstall\openFatt"

Name "openFatt ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\openFatt"
InstallDirRegKey HKCU "Software\openFatt" "InstallDir"
BrandingText "openFatt ${VERSION}"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING

!define MUI_WELCOMEPAGE_TITLE "Installazione di openFatt ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "openFatt è un gestionale open source per fatture, magazzino e fatture elettroniche.$\r$\n$\r$\nSe openFatt è già installato, verrà aggiornato: archivi, impostazioni e copie di sicurezza restano al loro posto.$\r$\n$\r$\nChiudere openFatt prima di continuare."
!insertmacro MUI_PAGE_WELCOME
!define MUI_LICENSEPAGE_TEXT_TOP "openFatt è distribuito con licenza AGPL-3.0, senza alcuna garanzia."
!define MUI_LICENSEPAGE_TEXT_BOTTOM "Per continuare occorre accettare la licenza."
!insertmacro MUI_PAGE_LICENSE "${LICENSE}"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\openfatt.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Avvia openFatt"
!insertmacro MUI_PAGE_FINISH

!define MUI_UNCONFIRMPAGE_TEXT_TOP "Viene rimosso il solo programma. Archivi e copie di sicurezza in Documenti\openFatt non vengono cancellati."
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "Italian"

VIProductVersion "${VERSION}.0"
VIFileVersion "${VERSION}.0"
VIAddVersionKey /LANG=${LANG_ITALIAN} "ProductName" "openFatt"
VIAddVersionKey /LANG=${LANG_ITALIAN} "FileDescription" "Installazione di openFatt"
VIAddVersionKey /LANG=${LANG_ITALIAN} "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ITALIAN} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ITALIAN} "LegalCopyright" "openFatt contributors - AGPL-3.0-or-later"

; Il programma aperto blocca il proprio file: finché non viene chiuso non si può sostituire.
!macro ATTENDI_CHIUSURA
    StrCpy $Attese 0
    riprova:
    IfFileExists "$INSTDIR\openfatt.exe" 0 libero
    ClearErrors
    FileOpen $0 "$INSTDIR\openfatt.exe" a
    IfErrors occupato
    FileClose $0
    Goto libero
    occupato:
    StrCmp $Riavvia "1" 0 chiedi
    ; il programma si sta chiudendo: si attende fino a un minuto
    IntOp $Attese $Attese + 1
    IntCmp $Attese 120 fallito 0 fallito
    Sleep 500
    Goto riprova
    chiedi:
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "openFatt è ancora aperto. Chiuderlo e premere Riprova." /SD IDCANCEL IDRETRY riprova
    fallito:
    Abort
    libero:
!macroend

Function .onInit
    ${GetParameters} $0
    ClearErrors
    ${GetOptions} $0 "/RIAVVIA" $1
    IfErrors +2
    StrCpy $Riavvia "1"
FunctionEnd

Section "openFatt"
    SetOutPath "$INSTDIR"
    !insertmacro ATTENDI_CHIUSURA
    ; la versione precedente si tiene da parte finché la nuova non è al suo posto
    Delete "$INSTDIR\openfatt.exe.old"
    Rename "$INSTDIR\openfatt.exe" "$INSTDIR\openfatt.exe.old"
    ClearErrors
    File /nonfatal "/oname=openfatt.exe" "${EXE}"
    IfErrors 0 copiato
    Delete "$INSTDIR\openfatt.exe"
    Rename "$INSTDIR\openfatt.exe.old" "$INSTDIR\openfatt.exe"
    StrCmp $Riavvia "1" 0 +2
    Exec '"$INSTDIR\openfatt.exe"'
    Abort
    copiato:
    Delete "$INSTDIR\openfatt.exe.old"
    File "/oname=openfatt-easyfatt.exe" "${HELPER}"
    File "/oname=openfatt.ico" "${ICON}"
    File "/oname=LICENSE.txt" "${LICENSE}"
    WriteUninstaller "$INSTDIR\Disinstalla.exe"

    !ifndef ISOLATO
    SetShellVarContext current
    CreateShortcut "$SMPROGRAMS\openFatt.lnk" "$INSTDIR\openfatt.exe" "" "$INSTDIR\openfatt.ico"
    CreateShortcut "$DESKTOP\openFatt.lnk" "$INSTDIR\openfatt.exe" "" "$INSTDIR\openfatt.ico"

    WriteRegStr HKCU "Software\openFatt" "InstallDir" "$INSTDIR"
    WriteRegStr HKCU "${CHIAVE}" "DisplayName" "openFatt"
    WriteRegStr HKCU "${CHIAVE}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "${CHIAVE}" "Publisher" "openFatt contributors"
    WriteRegStr HKCU "${CHIAVE}" "URLInfoAbout" "https://github.com/m1k3-shell/openFatt"
    WriteRegStr HKCU "${CHIAVE}" "DisplayIcon" "$INSTDIR\openfatt.ico"
    WriteRegStr HKCU "${CHIAVE}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "${CHIAVE}" "UninstallString" '"$INSTDIR\Disinstalla.exe"'
    WriteRegStr HKCU "${CHIAVE}" "QuietUninstallString" '"$INSTDIR\Disinstalla.exe" /S'
    WriteRegDWORD HKCU "${CHIAVE}" "NoModify" 1
    WriteRegDWORD HKCU "${CHIAVE}" "NoRepair" 1
    !endif

    StrCmp $Riavvia "1" 0 +2
    Exec '"$INSTDIR\openfatt.exe"'
SectionEnd

Section "Uninstall"
    !insertmacro ATTENDI_CHIUSURA
    Delete "$INSTDIR\openfatt-easyfatt.exe"
    Delete "$INSTDIR\openfatt.ico"
    Delete "$INSTDIR\LICENSE.txt"
    Delete "$INSTDIR\Disinstalla.exe"
    Delete "$INSTDIR\openfatt.exe.old"
    RMDir "$INSTDIR"

    !ifndef ISOLATO
    SetShellVarContext current
    Delete "$SMPROGRAMS\openFatt.lnk"
    Delete "$DESKTOP\openFatt.lnk"

    DeleteRegKey HKCU "${CHIAVE}"
    DeleteRegKey HKCU "Software\openFatt"
    !endif
SectionEnd
