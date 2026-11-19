# Draws the Stream Deck plugin's key images and icons, in the app's own look:
# each key is one of Hindsight's buttons (Save clip is the white one, the rest
# are dark), with the app's icons (Lucide) and colours.
#   powershell -ExecutionPolicy Bypass -File scripts\make-streamdeck-icons.ps1
# Uses WPF (needs a single-threaded apartment, the default for Windows PowerShell).
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName PresentationCore, PresentationFramework, WindowsBase
$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root "streamdeck\com.rewind.clipper.sdPlugin\imgs"
New-Item -ItemType Directory -Force $out | Out-Null

# The app's colours (src/App.css): page, raised button, its edge, accent, record red.
$page = '#181818'; $raised = '#2a2a2a'; $edge = '#3a3a3a'; $accent = '#4cc2ff'; $rec = '#ff5c5c'

function Brush($hex, $alpha = 255) {
    $c = [Windows.Media.ColorConverter]::ConvertFromString($hex); $c.A = $alpha
    $b = New-Object Windows.Media.SolidColorBrush $c; $b.Freeze(); $b
}
# Lucide icons (24 x 24 grid, round 2 px strokes), the same set the app uses.
$ICON = @{
    history    = 'M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8 M3 3v5h5 M12 7v5l4 2'
    pause      = 'M7 4h2a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z M15 4h2a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1h-2a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z'
    monitorcog = 'M12 17v4 M15.2 4.9l-.9-.4 M15.2 7.1l-.9.4 M16.9 3.2l-.4-.9 M16.9 8.8l-.4.9 M19.5 2.3l-.4.9 M19.5 9.7l-.4-.9 M21.7 4.5l-.9.4 M21.7 7.5l-.9-.4 M22 13v2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7 M8 21h8 M21 6a3 3 0 1 1-6 0 3 3 0 1 1 6 0z'
    circledot  = 'M22 12a10 10 0 1 1-20 0 10 10 0 1 1 20 0z M13 12a1 1 0 1 1-2 0 1 1 0 1 1 2 0z'
}
# One icon, `size` wide, centred on (cx, cy), in the 144 px drawing space.
function Glyph($dc, $name, $cx, $cy, $size, $brush, $stroke = 2) {
    $g = [Windows.Media.Geometry]::Parse($ICON[$name]).Clone()
    $s = $size / 24
    $t = New-Object Windows.Media.TransformGroup
    $t.Children.Add((New-Object Windows.Media.ScaleTransform $s, $s))
    $t.Children.Add((New-Object Windows.Media.TranslateTransform ($cx - $size / 2), ($cy - $size / 2)))
    $g.Transform = $t
    $p = New-Object Windows.Media.Pen $brush, ($stroke * $s)
    $p.StartLineCap = 'Round'; $p.EndLineCap = 'Round'; $p.LineJoin = 'Round'
    $dc.DrawGeometry($null, $p, $g)
}
function Dot($dc, $brush, $r, $pen = $null) { $dc.DrawEllipse($brush, $pen, (New-Object Windows.Point 72, 58), $r, $r) }
# The button the key shows, above the space Stream Deck keeps for the title.
function Tile($dc, $fill, $line) {
    $pen = if ($line) { New-Object Windows.Media.Pen (Brush $line), 2 } else { $null }
    $dc.DrawRoundedRectangle((Brush $fill), $pen, (New-Object Windows.Rect 14, 10, 116, 96), 22, 22)
}
# Draw in a 144 px space, saved at `px` (drawn at that size, not scaled down after).
function Render($file, $px, $bg, [scriptblock]$draw, $space = 144) {
    $v = New-Object Windows.Media.DrawingVisual
    $dc = $v.RenderOpen()
    $dc.PushTransform((New-Object Windows.Media.ScaleTransform ($px / $space), ($px / $space)))
    if ($bg) { $dc.DrawRectangle((Brush $bg), $null, (New-Object Windows.Rect 0, 0, $space, $space)) }
    & $draw $dc
    $dc.Pop(); $dc.Close()
    $bmp = New-Object Windows.Media.Imaging.RenderTargetBitmap $px, $px, 96, 96, ([Windows.Media.PixelFormats]::Pbgra32)
    $bmp.Render($v)
    $enc = New-Object Windows.Media.Imaging.PngBitmapEncoder
    $enc.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($bmp))
    $fs = [IO.File]::Create((Join-Path $out "$file.png")); try { $enc.Save($fs) } finally { $fs.Close() }
}
# Stream Deck picks name.png for standard keys and name@2x.png for high-DPI ones.
function Key($name, [scriptblock]$draw) {
    Render $name 72 $page $draw
    Render "$name@2x" 144 $page $draw
}

Key "key-save" { param($dc) Tile $dc '#ffffff'; Glyph $dc 'history' 72 58 56 (Brush '#111111') 2.2 }
Key "key-record" { param($dc) Tile $dc $raised $edge; Dot $dc (Brush $rec) 22 }
Key "key-record-on" { param($dc)
    Tile $dc '#4a1a1a' $rec
    Dot $dc $null 30 (New-Object Windows.Media.Pen (Brush $rec 110), 6)
    Dot $dc (Brush $rec) 22 }
Key "key-buffer-on" { param($dc) Tile $dc $raised $edge; Glyph $dc 'history' 72 58 54 (Brush $accent) 2.2 }
Key "key-buffer-off" { param($dc) Tile $dc $raised $edge; Glyph $dc 'pause' 72 58 48 (Brush '#8a8a8a') 2.2 }
Key "key-quality" { param($dc) Tile $dc $raised $edge; Glyph $dc 'monitorcog' 72 58 54 (Brush '#ffffff') 2 }

# Action-list and category icons: white line art on transparent (20 px, and 40 px @2x),
# the same icons as the keys.
function Icon($name, $glyph) {
    $script:iconGlyph = $glyph
    $draw = { param($dc) Glyph $dc $script:iconGlyph 72 72 128 (Brush '#ffffff') 2 }
    Render $name 20 $null $draw
    Render "$name@2x" 40 $null $draw
}
Icon "action-save" 'history'
Icon "action-record" 'circledot'
Icon "action-buffer" 'pause'
Icon "action-quality" 'monitorcog'
Icon "category" 'history'

# Plugin icon (store and plugin list): the app logo, 144 px and 288 px @2x.
$logo = New-Object Windows.Media.Imaging.BitmapImage
$logo.BeginInit(); $logo.UriSource = New-Object Uri (Join-Path $root "src-tauri\icons\icon.png"); $logo.CacheOption = 'OnLoad'; $logo.EndInit()
foreach ($p in @(@("plugin", 144), @("plugin@2x", 288))) {
    Render $p[0] $p[1] $null { param($dc) $dc.DrawImage($logo, (New-Object Windows.Rect 0, 0, 144, 144)) }
}
Get-ChildItem $out | Select-Object Name, Length
