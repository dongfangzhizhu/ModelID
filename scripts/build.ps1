##############################################################################
# Build script for ModelID Rust projects (Windows PowerShell)
# Supports Windows environments
# Usage:
#   .\scripts\build.ps1              # Build all crates
#   .\scripts\build.ps1 modeld-core  # Build specific crate
#   .\scripts\build.ps1 -Help        # Show help
##############################################################################

param(
    [Parameter(ValueFromRemainingArguments=$true)]
    [string[]]$Arguments
)

# Available crates
$CRATES = @(
    "modeld-core",
    "modeld-cli",
    "modeld-proxy",
    "modeld-client",
    "modeld-webui"
)

# Default values
$BuildType = "debug"
$ReleaseMode = $false
$TargetCrates = @()
$Verbose = $false
$Action = "build"
$ShowHelp = $false

# Color codes
$ColorRed = "Red"
$ColorGreen = "Green"
$ColorYellow = "Yellow"
$ColorBlue = "Cyan"

# Print help
function Print-Help {
    Write-Host ""
    Write-Host "Usage: build.ps1 [OPTIONS] [CRATE_NAME]" -ForegroundColor $ColorBlue
    Write-Host ""
    Write-Host "Options:" -ForegroundColor $ColorBlue
    Write-Host "    -Release            Build in release mode (optimized)"
    Write-Host "    -Verbose            Show detailed build output"
    Write-Host "    -Check              Only check code without building"
    Write-Host "    -Test               Build and run tests"
    Write-Host "    -All                Build all crates (default if no crate specified)"
    Write-Host "    -Help               Show this help message"
    Write-Host ""
    Write-Host "Crate Names:" -ForegroundColor $ColorBlue
    foreach ($crate in $CRATES) {
        Write-Host "    - $crate"
    }
    Write-Host ""
    Write-Host "Examples:" -ForegroundColor $ColorBlue
    Write-Host "    # Build all crates in debug mode"
    Write-Host "    .\scripts\build.ps1"
    Write-Host ""
    Write-Host "    # Build specific crate in release mode"
    Write-Host "    .\scripts\build.ps1 -Release modeld-core"
    Write-Host ""
    Write-Host "    # Build and test"
    Write-Host "    .\scripts\build.ps1 -Test"
    Write-Host ""
    Write-Host "    # Only check without building"
    Write-Host "    .\scripts\build.ps1 -Check"
    Write-Host ""
}

# Print colored output
function Print-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor $ColorBlue
}

function Print-Success {
    param([string]$Message)
    Write-Host "[SUCCESS] $Message" -ForegroundColor $ColorGreen
}

function Print-Error {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor $ColorRed
}

function Print-Warn {
    param([string]$Message)
    Write-Host "[WARN] $Message" -ForegroundColor $ColorYellow
}

# Check if crate name is valid
function Test-ValidCrate {
    param([string]$CrateName)
    return $CRATES -contains $CrateName
}

# Build crate
function Invoke-BuildCrate {
    param(
        [string]$CrateName,
        [bool]$Release,
        [bool]$Verbose
    )
    
    $cargoArgs = @("build", "-p", $CrateName)
    
    if ($Verbose) {
        $cargoArgs += "--verbose"
    }
    
    if ($Release) {
        $cargoArgs += "--release"
    }

    Print-Info "Building $CrateName..."
    & cargo @cargoArgs
    
    if ($LASTEXITCODE -eq 0) {
        Print-Success "Built $CrateName successfully"
        return $true
    } else {
        Print-Error "Failed to build $CrateName"
        return $false
    }
}

# Check crate
function Invoke-CheckCrate {
    param(
        [string]$CrateName,
        [bool]$Release,
        [bool]$Verbose
    )
    
    $cargoArgs = @("check", "-p", $CrateName)
    
    if ($Verbose) {
        $cargoArgs += "--verbose"
    }
    
    if ($Release) {
        $cargoArgs += "--release"
    }

    Print-Info "Checking $CrateName..."
    & cargo @cargoArgs
    
    if ($LASTEXITCODE -eq 0) {
        Print-Success "Checked $CrateName successfully"
        return $true
    } else {
        Print-Error "Check failed for $CrateName"
        return $false
    }
}

# Test crate
function Invoke-TestCrate {
    param(
        [string]$CrateName,
        [bool]$Verbose
    )
    
    $cargoArgs = @("test", "-p", $CrateName)
    
    if ($Verbose) {
        $cargoArgs += "--verbose"
    }

    Print-Info "Testing $CrateName..."
    & cargo @cargoArgs
    
    if ($LASTEXITCODE -eq 0) {
        Print-Success "Tested $CrateName successfully"
        return $true
    } else {
        Print-Error "Tests failed for $CrateName"
        return $false
    }
}

# Parse arguments
$i = 0
while ($i -lt $Arguments.Count) {
    $arg = $Arguments[$i]
    
    switch -CaseSensitive ($arg) {
        "-Help" {
            $ShowHelp = $true
            $i++
            break
        }
        "-Release" {
            $ReleaseMode = $true
            $BuildType = "release"
            $i++
        }
        "-Verbose" {
            $Verbose = $true
            $i++
        }
        "-Check" {
            $Action = "check"
            $i++
        }
        "-Test" {
            $Action = "test"
            $i++
        }
        "-All" {
            $TargetCrates = $CRATES
            $i++
        }
        default {
            if (Test-ValidCrate $arg) {
                $TargetCrates += $arg
            } else {
                Print-Error "Unknown crate or option: $arg"
                Print-Help
                exit 1
            }
            $i++
        }
    }
}

# Show help if requested
if ($ShowHelp) {
    Print-Help
    exit 0
}

# If no crates specified, build all
if ($TargetCrates.Count -eq 0) {
    $TargetCrates = $CRATES
}

Print-Info "Starting build process..."
Print-Info "Build type: $BuildType"
Print-Info "Action: $Action"
Print-Info "Crates to process: $($TargetCrates -join ', ')"
Write-Host ""

$successfulCrates = @()
$failedCrates = @()

# Build/check/test each crate
foreach ($crate in $TargetCrates) {
    $result = $false
    
    switch ($Action) {
        "check" {
            $result = Invoke-CheckCrate -CrateName $crate -Release $ReleaseMode -Verbose $Verbose
        }
        "test" {
            $result = Invoke-TestCrate -CrateName $crate -Verbose $Verbose
        }
        default {
            $result = Invoke-BuildCrate -CrateName $crate -Release $ReleaseMode -Verbose $Verbose
        }
    }

    if ($result) {
        $successfulCrates += $crate
    } else {
        $failedCrates += $crate
    }
    Write-Host ""
}

# Print summary
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
Print-Info "Build Summary"
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if ($successfulCrates.Count -gt 0) {
    Print-Success "Successful: $($successfulCrates.Count) crate(s)"
    foreach ($crate in $successfulCrates) {
        Write-Host "  ✓ $crate"
    }
}

if ($failedCrates.Count -gt 0) {
    Print-Error "Failed: $($failedCrates.Count) crate(s)"
    foreach ($crate in $failedCrates) {
        Write-Host "  ✗ $crate"
    }
    Write-Host ""
    exit 1
}

Print-Success "All crates completed successfully!"
Write-Host ""
