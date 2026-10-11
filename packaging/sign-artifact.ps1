param([Parameter(Mandatory = $true)][string]$Artifact)

# Inno invokes this for Setup and its embedded signed uninstaller. Public
# certificate metadata travels through the build environment; keys stay in My.
$ErrorActionPreference = 'Stop'
$signTool = $env:UNLINKA_SIGN_TOOL
if (-not $signTool -or -not $env:UNLINKA_SIGN_THUMBPRINT -or -not $env:UNLINKA_SIGN_TIMESTAMP) {
    throw 'Signing build environment is incomplete'
}
& $signTool sign /fd SHA256 /sha1 $env:UNLINKA_SIGN_THUMBPRINT /s My /tr $env:UNLINKA_SIGN_TIMESTAMP /td SHA256 $Artifact
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $signTool verify /pa /all $Artifact
exit $LASTEXITCODE
