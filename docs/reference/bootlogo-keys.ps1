param([string]$Out)
# 800x480 boot logo laid out on the deck's 5x3 key grid (160 px cells): one glyph per key.
Add-Type -AssemblyName System.Drawing
$W = 800; $H = 480; $CELL = 160
$bmp = New-Object System.Drawing.Bitmap $W, $H
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'
$g.Clear([System.Drawing.Color]::FromArgb(8, 9, 11))

$orange = [System.Drawing.Color]::FromArgb(255, 154, 60)
$white = [System.Drawing.Color]::FromArgb(238, 240, 242)
$fmt = New-Object System.Drawing.StringFormat
$fmt.Alignment = 'Center'; $fmt.LineAlignment = 'Center'

function Glow($cx, $cy, $r, $alpha) {
  for ($i = $r; $i -gt 0; $i -= 4) {
    $a = [int]($alpha * (1 - $i / $r))
    $b = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb($a, 255, 154, 60))
    $g.FillEllipse($b, $cx - $i, $cy - $i, 2 * $i, 2 * $i)
  }
}
function Letter($col, $row, $text, $color, $size) {
  $cx = $col * $CELL + $CELL / 2; $cy = $row * $CELL + $CELL / 2
  $font = New-Object System.Drawing.Font 'Segoe UI', $size, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
  $brush = New-Object System.Drawing.SolidBrush $color
  $g.DrawString($text, $font, $brush, (New-Object System.Drawing.RectangleF ($cx - 80), ($cy - 80 + 4), 160, 160), $fmt)
}
function RoundRect($x, $y, $w, $h, $r) {
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $p.AddArc($x, $y, 2*$r, 2*$r, 180, 90); $p.AddArc($x+$w-2*$r, $y, 2*$r, 2*$r, 270, 90)
  $p.AddArc($x+$w-2*$r, $y+$h-2*$r, 2*$r, 2*$r, 0, 90); $p.AddArc($x, $y+$h-2*$r, 2*$r, 2*$r, 90, 90)
  $p.CloseFigure(); return $p
}

# row 1: GERZY
$i = 0; foreach ($ch in 'G','E','R','Z','Y') { Letter $i 0 $ch $white 84; $i++ }
# row 2: app icon in the middle key (3x2 keys, one lit)
$cx = 2 * $CELL + $CELL / 2; $cy = $CELL + $CELL / 2
Glow $cx ($cy - 12) 46 70
$k = 22; $gap = 6; $x0 = $cx - (3 * $k + 2 * $gap) / 2; $y0 = $cy - ($k * 2 + $gap) / 2
$grey = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(70, 76, 84))
$lit = New-Object System.Drawing.SolidBrush $orange
for ($r = 0; $r -lt 2; $r++) { for ($c = 0; $c -lt 3; $c++) {
  $b = if ($r -eq 0 -and $c -eq 1) { $lit } else { $grey }
  $g.FillPath($b, (RoundRect ($x0 + $c * ($k + $gap)) ($y0 + $r * ($k + $gap)) $k $k 6))
} }
# row 3: DECK + D6
$i = 0; foreach ($ch in 'D','E','C','K') { Letter $i 2 $ch $orange 84; $i++ }
Letter 4 2 'D6' $white 46

$bmp.Save("$Out.png", [System.Drawing.Imaging.ImageFormat]::Png)
# guide overlay (not uploaded): the 160 px cells and a 104 px key area
$guide = $bmp.Clone()
$gg = [System.Drawing.Graphics]::FromImage($guide)
$pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(90, 255, 255, 255)), 1
$pen2 = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(120, 0, 200, 255)), 1
for ($r = 0; $r -lt 3; $r++) { for ($c = 0; $c -lt 5; $c++) {
  $gg.DrawRectangle($pen, $c * $CELL, $r * $CELL, $CELL - 1, $CELL - 1)
  $gg.DrawRectangle($pen2, $c * $CELL + 28, $r * $CELL + 28, 104, 104)
} }
$guide.Save("$Out-guide.png", [System.Drawing.Imaging.ImageFormat]::Png)
"saved"
