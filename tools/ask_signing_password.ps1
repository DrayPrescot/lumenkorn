# Fragt für tools/release_local.mjs in einem eigenen Fenster nach Schlüsseldatei und
# Passwort. Antwort auf stdout: Base64 von UTF-8-JSON, Exit 3 bei Abbruch. Nach 30 Minuten
# schließt sich das Fenster selbst, damit keines übrig bleibt, wenn der Aufrufer beendet wurde.
#
# Umlaute in Texten nur über [char]: Windows PowerShell liest Skripte ohne BOM nicht als UTF-8.
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$KeyPath = "",
  [string]$Message = ""
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[Windows.Forms.Application]::EnableVisualStyles()

$ue = [char]0xFC
$form = New-Object Windows.Forms.Form
$form.Text = "Lumenkorn $Version signieren"
$form.ClientSize = New-Object Drawing.Size(480, 178)
$form.Font = New-Object Drawing.Font("Segoe UI", 9)
$form.FormBorderStyle = "FixedDialog"
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.StartPosition = "CenterScreen"
$form.TopMost = $true

function Add-Control($type, $x, $y, $w, $h, $text = "") {
  $control = New-Object "Windows.Forms.$type"
  $control.SetBounds($x, $y, $w, $h)
  $control.Text = $text
  $form.Controls.Add($control)
  return $control
}

$intro = "Lumenkorn $Version wartet auf die Signatur. Das Passwort geht nur an den Signierschritt und wird nirgends gespeichert oder angezeigt."
[void](Add-Control Label 12 12 456 36 $intro)
[void](Add-Control Label 12 60 92 20 "Schl${ue}sseldatei:")
$keyBox = Add-Control TextBox 106 57 276 23 $KeyPath
$browse = Add-Control Button 388 56 80 25 "Suchen..."
[void](Add-Control Label 12 90 92 20 "Passwort:")
$passwordBox = Add-Control TextBox 106 87 362 23
$passwordBox.UseSystemPasswordChar = $true
$note = Add-Control Label 12 116 456 20 $Message
$note.ForeColor = [Drawing.Color]::Firebrick
$sign = Add-Control Button 302 144 80 26 "Signieren"
$cancel = Add-Control Button 388 144 80 26 "Abbrechen"
$form.AcceptButton = $sign
$form.CancelButton = $cancel
$cancel.DialogResult = "Cancel"

$browse.Add_Click({
  $picker = New-Object Windows.Forms.OpenFileDialog
  $picker.Filter = "Schl${ue}ssel (*.key)|*.key|Alle Dateien (*.*)|*.*"
  if ($picker.ShowDialog($form) -eq "OK") { $keyBox.Text = $picker.FileName }
})

$sign.Add_Click({
  if (-not $keyBox.Text -or -not (Test-Path -LiteralPath $keyBox.Text -PathType Leaf)) {
    $note.Text = "Schl${ue}sseldatei nicht gefunden."
    [void]$keyBox.Focus()
  } elseif (-not $passwordBox.Text) {
    $note.Text = "Bitte das Passwort eingeben."
    [void]$passwordBox.Focus()
  } else {
    $form.DialogResult = "OK"
  }
})

$form.Add_Shown({
  $form.Activate()
  [void]$passwordBox.Focus()
})

$timer = New-Object Windows.Forms.Timer
$timer.Interval = 30 * 60 * 1000
$timer.Add_Tick({ $form.Close() })
$timer.Start()

$result = $form.ShowDialog()
$timer.Stop()
if ($result -ne "OK") { exit 3 }

$answer = @{ key = $keyBox.Text; password = $passwordBox.Text } | ConvertTo-Json -Compress
[Console]::Out.Write([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($answer)))
exit 0
