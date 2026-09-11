# Captures the whole desktop from the console session (TASK-138).
#
# The helper runs a queued script with no arguments and no inherited
# environment, so the caller names the file by prepending a line that sets
# $ShotName. See the workflow.
#
# A screenshot taken from session 0 is a black rectangle, which is the failure
# this whole image exists to avoid, so this also counts distinct colours and
# fails when the picture is effectively blank.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

if (-not $ShotName) { $ShotName = 'shot' }
$name = $ShotName
$dir = 'C:\SubordinateTest\shots'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$path = Join-Path $dir "$name.png"

$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)

$colors = @{}
for ($x = 0; $x -lt $b.Width; $x += 16) {
  for ($y = 0; $y -lt $b.Height; $y += 16) {
    $colors[$bmp.GetPixel($x, $y).ToArgb()] = 1
  }
}
$g.Dispose()
$bmp.Dispose()
"saved $path ($((Get-Item $path).Length) bytes) from a $($b.Width)x$($b.Height) desktop, $($colors.Count) distinct sampled colours"
if ($colors.Count -lt 8) { throw 'the desktop is effectively blank' }
