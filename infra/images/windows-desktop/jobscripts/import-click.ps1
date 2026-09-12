# Runs import-click.py in the console session, photographs whatever came up,
# and closes the dialog again (TASK-138).
#
# The caller stages import-click.py at C:\SubordinateTest\import-click.py.
$ErrorActionPreference = 'Continue'
$python = 'C:\Program Files\Python312\python.exe'
if (-not (Test-Path $python)) { throw "no machine-wide python at $python" }

& $python C:\SubordinateTest\import-click.py
$code = $LASTEXITCODE
"import-click.py exit=$code"

# Shoot the screen either way: the dialog when it opened, the editor when it
# did not. A picture of the failure is worth more than the exit code.
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save('C:\SubordinateTest\shots\02-import-dialog.png', [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
'saved 02-import-dialog.png'

# Leave the editor usable for the MCP check: escape closes the file dialog.
[System.Windows.Forms.SendKeys]::SendWait('{ESC}')
Start-Sleep 2
exit $code
