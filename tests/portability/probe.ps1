# Windows-native probe (PowerShell 7). Prints PASS/FAIL/INFO lines; never stops early.
$ErrorActionPreference = 'Continue'
$cig = $args[0]
$root = Join-Path $env:RUNNER_TEMP ("ps" + (Get-Random))
New-Item -ItemType Directory $root | Out-Null
$env:NO_COLOR = '1'
$env:CIG_SEAL_KEY = 'file'
$script:pass = 0; $script:fail = 0

function Fresh {
  $d = Join-Path $root ("s" + (Get-Random))
  New-Item -ItemType Directory $d, "$d-home" | Out-Null
  $env:CIGSCRIPT_HOME = "$d-home"; Set-Location $d
}
function Cig { $script:out = (& $cig @args 2>&1 | Out-String); $script:code = $LASTEXITCODE }
function Short($s) { $t = ($s -replace '\s+', ' ').Trim(); if ($t.Length -gt 170) { $t.Substring(0, 170) } else { $t } }
function Result($name, $ok, $detail) {
  if ($ok) { $s = 'PASS'; $script:pass++ } else { $s = 'FAIL'; $script:fail++ }
  "{0,-4} {1,-52} {2}" -f $s, $name, (Short $detail)
}
function Info($name, $detail) { "{0,-4} {1,-52} {2}" -f 'INFO', $name, (Short $detail) }
function Script($text) { [IO.File]::WriteAllText("$PWD\s.cig", $text) }

"== windows probe: $([Environment]::OSVersion.VersionString) $env:PROCESSOR_ARCHITECTURE, $(& $cig --version)"

# Program lookup: bare names need PATHEXT (.exe, .cmd, .bat)
$progs = @(
  @('powershell', '"-NoProfile", "-Command", "exit 0"'),
  @('powershell.exe', '"-NoProfile", "-Command", "exit 0"'),
  @('pwsh', '"-NoProfile", "-Command", "exit 0"'),
  @('cmd', '"/c", "exit 0"'),
  @('node', '"--version"'),
  @('npm', '"--version"'),
  @('npm.cmd', '"--version"'),
  @('python', '"--version"'),
  @('git', '"--version"'),
  @('where', '"where"')
)
foreach ($p in $progs) {
  Fresh; Script "burn {`n  stick r = proc.run(`"$($p[0])`", [$($p[1])])`n  exhale r.code`n}`n"
  Cig run s.cig
  Result "proc.run finds ``$($p[0])``" ($code -eq 0) $out
}

# Which bash does a bare name start? (System32\bash.exe is the WSL launcher)
Fresh; Script "exhale proc.which(`"bash`") ?? `"not found`"`nburn {`n  stick r = proc.run(`"bash`", [`"-c`", `"echo bash=`$BASH_VERSION`"], {ok: [0, 1, -1]})`n  exhale r.out`n  exhale r.err`n}`n"
Cig run s.cig; Info "bash: which one runs" "exit $code; $out"

# Changing an existing file (snapshot + flush of the before-image)
Fresh; Set-Content keep.txt 'original'
Script "burn {`n  fs.write_text(`"keep.txt`", `"changed`")`n}`n"
Cig run s.cig; Result "a burn can change an existing file" (($code -eq 0) -and ((Get-Content keep.txt) -eq 'changed')) "exit $code; $out"

# Where does a bare name resolve? (which, if the stdlib has it)
Fresh; Script "exhale proc.which(`"npm`") ?? `"not found`"`nexhale proc.which(`"node`") ?? `"not found`"`n"
Cig run s.cig; Info "proc.which npm / node" $out

# A script saved with a BOM (Notepad, Set-Content -Encoding utf8BOM)
Fresh; [IO.File]::WriteAllText("$PWD\s.cig", "exhale `"bom ok`"`r`n", (New-Object Text.UTF8Encoding($true)))
Cig run s.cig; Result "a script with a UTF-8 BOM parses" ($out -match 'bom ok') $out

# Environment variable names are case-insensitive on Windows
Fresh; Script "exhale env.get(`"Path`") != null`nexhale env.get(`"PATH`") != null`n"
Cig run s.cig; Result "env.get is case-insensitive (Path / PATH)" (($out -split "`n" | Where-Object { $_ -match 'true' }).Count -eq 2) $out

# Backslash paths inside a script
Fresh; Script "burn {`n  fs.mkdir(`"sub`")`n  fs.write_text(`"sub\\x.txt`", `"1`")`n  cough `"boom`"`n}`n"
Cig run s.cig; Result "backslash path rolls back" (-not (Test-Path sub)) $out

# Pack containment: same folder spelled with different case is inside
Fresh; $here = (Get-Location).Path; $flip = $here.Substring(0,1).ToLower() + $here.Substring(1).ToUpper()
Script "pack { `".`" }`nburn {`n  fs.write_text(`"$($flip -replace '\\','\\')\\c.txt`", `"1`")`n}`n"
Cig run s.cig; Result "pack: same folder, different case, is inside" ($code -eq 0) $out

# Pack containment: another drive path is outside
Fresh; Script "pack { `".`" }`nburn {`n  fs.write_text(`"C:\\Windows\\Temp\\cigprobe-$(Get-Random).txt`", `"1`")`n}`n"
Cig run s.cig; Result "pack: a C:\ path outside the pack is refused" ($code -ne 0) $out

# Reserved device names
Fresh; Script "burn {`n  fs.write_text(`"aux.txt`", `"x`")`n  cough `"boom`"`n}`n"
Cig run s.cig; Info "write to aux.txt (reserved name) then cough" "exit $code; $out"
Fresh; Script "burn {`n  fs.write_text(`"nul`", `"x`")`n}`n"
Cig run s.cig; Info "write to nul" "exit $code; $out"

# A file another program holds open (an editor, an antivirus scan, a running exe)
Fresh; Set-Content a.txt 'original'
$h = [IO.File]::Open("$PWD\a.txt", 'Open', 'Read', 'ReadWrite')
Script "burn {`n  fs.write_text(`"a.txt`", `"changed`")`n  cough `"boom`"`n}`n"
Cig run s.cig; $h.Close()
Result "file held open (share read/write, no delete): rollback restores" ((Get-Content a.txt) -eq 'original') "exit $code; a.txt=$(Get-Content a.txt)"
"----- full output (file held open):"; $out; Cig --json runs; "----- runs: $out"

Fresh; Set-Content a.txt 'original'
$h = [IO.File]::Open("$PWD\a.txt", 'Open', 'Read', 'Read')
Script "burn {`n  fs.write_text(`"a.txt`", `"changed`")`n}`n"
Cig run s.cig; $h.Close()
Result "file locked for writing: refused cleanly, unchanged" (($code -ne 0) -and ((Get-Content a.txt) -eq 'original')) "exit $code; $out"

# Symlinks (runners are admin, so New-Item can make them)
Fresh; Set-Content target.txt 't'
try { New-Item -ItemType SymbolicLink -Path link.txt -Target target.txt -ErrorAction Stop | Out-Null; $made = $true } catch { $made = $false }
if ($made) {
  Script "burn {`n  fs.rm(`"link.txt`")`n  cough `"boom`"`n}`n"
  Cig run s.cig
  $lt = (Get-Item link.txt -ErrorAction SilentlyContinue).LinkType
  Result "deleted symlink restored as a symlink" ($lt -eq 'SymbolicLink') "LinkType=$lt; $out"
} else { Info "symlink" "could not create one" }

# A killed run is interrupted and can be unburned
Fresh; Set-Content keep.txt 'original'
Script "burn {`n  fs.write_text(`"keep.txt`", `"changed`")`n  proc.run(`"powershell.exe`", [`"-NoProfile`", `"-Command`", `"Start-Sleep 30`"])`n}`n"
$p = Start-Process -FilePath $cig -ArgumentList 'run', 's.cig' -PassThru -NoNewWindow -RedirectStandardOutput o.txt -RedirectStandardError e.txt
Start-Sleep 5; Stop-Process -Id $p.Id -Force; Start-Sleep 1
Cig runs; $runsOut = $out; $st = (($out -split "`n")[1] -split '\s+')[1]
$id = (Get-ChildItem "$env:CIGSCRIPT_HOME\runs" | Sort-Object Name | Select-Object -Last 1).Name
Cig unburn $id
Result "killed run: interrupted, then unburned" (($st -eq 'interrupted') -and ((Get-Content keep.txt) -eq 'original')) "status=$st keep=$(Get-Content keep.txt); runs: $runsOut | $out"

# Self-update: which asset would it fetch here?
Fresh; Cig update --check; Info "cig update --check" "exit $code; $out"

"== windows probe: $pass passed, $fail failed"
