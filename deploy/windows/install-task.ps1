<#
.SYNOPSIS
  misao のデーモンを、ログオン時に起動するタスクスケジューラのタスクとして登録する。
.DESCRIPTION
  misao-task.xml の __USER__ / __MISAO__ / __LOG__ を置き換えて、現在のユーザーのタスク "misao" を登録する。
  昇格 (管理者権限) は要らない。すぐ起動するときは -Start を付ける。
.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File install-task.ps1 -Start
#>
[CmdletBinding()]
param(
  [string]$MisaoPath = (Join-Path $PSScriptRoot 'misao.exe'),
  [string]$LogPath = (Join-Path $env:USERPROFILE '.misao\misao.log'),
  [string]$TaskName = 'misao',
  [switch]$Start
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $MisaoPath -PathType Leaf)) {
  $found = Get-Command misao.exe -ErrorAction SilentlyContinue
  if ($null -eq $found) { throw "misao.exe が見つかりません。-MisaoPath で指定してください: $MisaoPath" }
  $MisaoPath = $found.Source
}
$MisaoPath = (Resolve-Path -LiteralPath $MisaoPath).Path
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogPath) | Out-Null

# XML の中に入れるので、& < > " などをエスケープする。
function Escape([string]$text) { [System.Security.SecurityElement]::Escape($text) }
$user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$template = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'misao-task.xml') -Raw -Encoding UTF8
$xml = $template.Replace('__USER__', (Escape $user)).Replace('__MISAO__', (Escape $MisaoPath)).Replace('__LOG__', (Escape $LogPath))
[void][xml]$xml  # 壊れた XML はここで止まる

Register-ScheduledTask -TaskName $TaskName -Xml $xml -Force | Out-Null
Write-Host "タスク '$TaskName' を登録しました（ユーザー: $user、ログ: $LogPath）"
if ($Start) {
  Start-ScheduledTask -TaskName $TaskName
  Write-Host "起動しました。確認: misao status"
}
