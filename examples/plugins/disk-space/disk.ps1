# Prints one JSON message per line. In poll mode the event that caused the
# run (if any) is in $env:NOOK_EVENT.
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8

function Emit($msg) { $msg | ConvertTo-Json -Depth 20 -Compress | Write-Output }

# A click on a drive row opens it in Explorer.
if ($env:NOOK_EVENT) {
    $ev = $env:NOOK_EVENT | ConvertFrom-Json
    if ($ev.type -eq 'event' -and $ev.id -like 'drive:*') {
        Emit @{ type = 'open'; target = ($ev.id.Substring(6) + '\') }
    }
}

$drives = Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | Sort-Object DeviceID
function Gb($b) { '{0:N0} GB' -f ($b / 1GB) }

# Tile: the system drive as a ring plus a line of text.
$sys = $drives | Where-Object DeviceID -eq $env:SystemDrive | Select-Object -First 1
if (-not $sys) { $sys = $drives | Select-Object -First 1 }
$used = 1 - $sys.FreeSpace / $sys.Size
$color = if ($used -gt 0.9) { 'red' } elseif ($used -gt 0.75) { 'orange' } else { 'accent' }
Emit @{
    type = 'tile'
    view = @{
        type = 'row'; gap = 10; align = 'center'
        children = @(
            @{ type = 'ring'; value = $used; label = ('{0:P0}' -f $used); size = 34; color = $color }
            @{ type = 'column'; gap = 0; children = @(
                @{ type = 'text'; text = "$($sys.DeviceID) free"; size = 'small'; color = 'dim'; bold = $true }
                @{ type = 'text'; text = (Gb $sys.FreeSpace); size = 'title' }
            ) }
        )
    }
}

# Page: every drive with a meter, clickable.
$rows = foreach ($d in $drives) {
    $u = 1 - $d.FreeSpace / $d.Size
    @{ type = 'card'; padding = 10; gap = 6; children = @(
        @{ type = 'row'; align = 'center'; children = @(
            @{ type = 'icon'; icon = 'hard-drive'; color = 'dim' }
            @{ type = 'text'; text = "$($d.DeviceID)  $($d.VolumeName)"; bold = $true }
            @{ type = 'spacer' }
            @{ type = 'button'; id = "drive:$($d.DeviceID)"; label = 'Open'; icon = 'folder-open' }
        ) }
        @{ type = 'progress'; value = $u; color = $(if ($u -gt 0.9) { 'red' } else { 'accent' }) }
        @{ type = 'text'; size = 'small'; color = 'dim';
           text = "$(Gb $d.FreeSpace) free of $(Gb $d.Size)" }
    ) }
}
Emit @{
    type = 'page'
    view = @{ type = 'column'; scroll = $true; gap = 8; children = @($rows) }
    actions = @(@{ id = 'refresh'; icon = 'arrow-clockwise'; tooltip = 'Refresh' })
}
