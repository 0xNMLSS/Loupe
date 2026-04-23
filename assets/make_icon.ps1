# Generate assets/lens.ico — a 256x256 magnifying-glass icon stored as a
# PNG-payload .ico (Vista+ supported). Run from the repository root:
#   powershell -ExecutionPolicy Bypass -File assets\make_icon.ps1

Add-Type -AssemblyName System.Drawing

$out = Join-Path (Split-Path -Parent $PSCommandPath) 'lens.ico'
$size = 256

$bmp = New-Object System.Drawing.Bitmap $size, $size
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode    = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.Clear([System.Drawing.Color]::Transparent)

# --- Lens body (translucent glass with crisp dark-blue ring) ---
$lensRect = New-Object System.Drawing.RectangleF 28, 28, 144, 144

# Soft glass fill with radial-ish gradient (simulated with two passes).
$glassFill = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(70, 140, 200, 255))
$g.FillEllipse($glassFill, $lensRect)
$glassFill.Dispose()

# Outer ring.
$ring = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 25, 70, 160)), 16
$g.DrawEllipse($ring, $lensRect)
$ring.Dispose()

# Inner highlight reflection — a thin arc near the upper-left.
$refl = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(220, 255, 255, 255)), 9
$refl.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$refl.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
$g.DrawArc($refl, 52, 52, 96, 96, 200, 70)
$refl.Dispose()

# --- Handle ---
$handle = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 25, 70, 160)), 30
$handle.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$handle.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
$g.DrawLine($handle, 168, 168, 232, 232)
$handle.Dispose()

# Subtle handle highlight stroke for depth.
$handleHi = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(140, 120, 170, 240)), 8
$handleHi.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$handleHi.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
$g.DrawLine($handleHi, 174, 174, 224, 224)
$handleHi.Dispose()

$g.Dispose()

# --- Encode as PNG, then wrap in an ICO directory entry ---
$ms = New-Object System.IO.MemoryStream
$bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
$png = $ms.ToArray()
$ms.Dispose()
$bmp.Dispose()

$icoMs = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $icoMs

# ICONDIR
$bw.Write([UInt16]0)         # reserved
$bw.Write([UInt16]1)         # type = ICO
$bw.Write([UInt16]1)         # image count

# ICONDIRENTRY (a 256-pixel dimension is encoded as 0)
$bw.Write([Byte]0)           # width
$bw.Write([Byte]0)           # height
$bw.Write([Byte]0)           # color count (0 = >=256 colors)
$bw.Write([Byte]0)           # reserved
$bw.Write([UInt16]1)         # color planes
$bw.Write([UInt16]32)        # bits per pixel
$bw.Write([UInt32]$png.Length)
$bw.Write([UInt32]22)        # data offset (6-byte header + 16-byte entry)

# PNG payload
$bw.Write($png)
$bw.Flush()

[System.IO.File]::WriteAllBytes($out, $icoMs.ToArray())
$icoMs.Dispose()

Write-Host "Wrote $out ($((Get-Item $out).Length) bytes)"
