<#
.SYNOPSIS
  install-task.ps1 で登録したタスクを外し、動いているデーモンを止める。
#>
[CmdletBinding()]
param(
  [string]$TaskName = 'misao',
  [string]$MisaoPath = ''
)
$ErrorActionPreference = 'Stop'

if ($MisaoPath -eq '') {
  $found = Get-Command misao.exe -ErrorAction SilentlyContinue
  if ($null -ne $found) { $MisaoPath = $found.Source }
}
# 先にタスクを外す（止めた直後に再起動されないように）。
if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
  Write-Host "タスク '$TaskName' を外しました"
}
if ($MisaoPath -ne '') {
  & $MisaoPath stop
  if ($LASTEXITCODE -ne 0) { Write-Host 'デーモンは動いていませんでした（または停止できませんでした）' }
}
