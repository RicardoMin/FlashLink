$ErrorActionPreference = 'Stop'
try {
    $gui = __GUI__
    $engine = __ENGINE__
    $base = __BASE__
    $rules = @(
        @{ Name='FlashLink.Nearby.UDP'; Program=$gui; Protocol='UDP'; LocalPort=@(47980) },
        @{ Name='FlashLink.Approval.TCP'; Program=$gui; Protocol='TCP'; LocalPort=@($base-8) },
        @{ Name='FlashLink.Stream.TCP'; Program=$engine; Protocol='TCP'; LocalPort=@(($base-5),$base,($base+21)) },
        @{ Name='FlashLink.Stream.UDP'; Program=$engine; Protocol='UDP'; LocalPort=@(($base+9),($base+10),($base+11)) }
    )
    foreach ($rule in $rules) {
        $old = Get-NetFirewallRule -Name $rule.Name -ErrorAction SilentlyContinue
        if ($old) { $old | Remove-NetFirewallRule }
        New-NetFirewallRule @rule -DisplayName $rule.Name -Direction Inbound -Profile Private -RemoteAddress LocalSubnet -Action Allow | Out-Null
    }
    $blocked = Get-NetFirewallRule -Enabled True -Action Block -Direction Inbound | Where-Object { $_.Profile -match 'Private|Any' } | Get-NetFirewallApplicationFilter | Where-Object { $_.Program -in @($gui,$engine) }
    if ($blocked) { exit 3 }
    if (-not (Get-NetConnectionProfile | Where-Object NetworkCategory -eq 'Private')) { exit 2 }
    exit 0
} catch { exit 1 }
