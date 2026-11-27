# One-shot frontend onboarding on Windows (C7).
param(
    [Parameter(Mandatory = $true)][ValidateSet('comfyui', 'forge', 'a1111')][string]$Frontend,
    [Parameter(Mandatory = $true)][string]$Target,
    [string]$Store = "$env:USERPROFILE\.modeld"
)

$ErrorActionPreference = 'Stop'

& modeld --store $Store frontend sync --frontend $Frontend
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& modeld --store $Store frontend connect $Frontend --target $Target
exit $LASTEXITCODE
