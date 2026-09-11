# Last screenshot, then close the editor (TASK-138). Runs in the console
# session; never fails the job.
$ErrorActionPreference = 'Continue'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save('C:\SubordinateTest\shots\03-final.png', [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
'saved 03-final.png'
if (Test-Path C:\SubordinateTest\app.pid) {
  $id = [int](Get-Content C:\SubordinateTest\app.pid -Raw).Trim()
  Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
  "stopped the editor (pid $id)"
}
