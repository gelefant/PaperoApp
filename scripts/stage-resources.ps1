#Requires -Version 5.1
param(
    [string]$OnnxRuntimeVersion = $(if ($env:ONNXRUNTIME_VERSION) { $env:ONNXRUNTIME_VERSION } else { "1.23.2" }),
    [string]$PdfiumBaseUrl = $(if ($env:PDFIUM_BASE_URL) { $env:PDFIUM_BASE_URL } else { "https://github.com/bblanchon/pdfium-binaries/releases/latest/download" }),
    [string]$DjvuBinDir = $env:DJVULIBRE_BIN_DIR
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir "..")).Path
$ResourcesDir = Join-Path $RepoRoot "src-tauri\resources"
$CacheDir = Join-Path $RepoRoot ".tmp_stage_cache"

Write-Host "==> Staging Papero native resources for Windows x64"

New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null
New-Item -ItemType Directory -Force -Path $ResourcesDir | Out-Null

# Clean staged items while keeping .gitkeep
Get-ChildItem -Path $ResourcesDir -Force | Where-Object { $_.Name -ne ".gitkeep" } | Remove-Item -Recurse -Force

$LicensesOut = Join-Path $ResourcesDir "licenses"
New-Item -ItemType Directory -Force -Path $LicensesOut | Out-Null

Copy-Item (Join-Path $RepoRoot "LICENSE-MIT") (Join-Path $LicensesOut "LICENSE-MIT")
Copy-Item (Join-Path $RepoRoot "LICENSE-APACHE") (Join-Path $LicensesOut "LICENSE-APACHE")
Copy-Item (Join-Path $RepoRoot "THIRD_PARTY_LICENSES.md") (Join-Path $LicensesOut "THIRD_PARTY_LICENSES.md")
Copy-Item (Join-Path $RepoRoot "licenses\GPL-2.0-DjVuLibre.txt") (Join-Path $LicensesOut "GPL-2.0-DjVuLibre.txt")
Copy-Item (Join-Path $RepoRoot "licenses\SOURCE_OFFER.txt") (Join-Path $LicensesOut "SOURCE_OFFER.txt")

function Download-Cached {
    param([string]$Url, [string]$Dest)
    if ((Test-Path $Dest) -and ((Get-Item $Dest).Length -gt 0)) {
        Write-Host "    [cache hit] $(Split-Path -Leaf $Dest)"
        return
    }
    Write-Host "    [download]  $Url"
    $Tmp = "$Dest.part"
    Invoke-WebRequest -Uri $Url -OutFile $Tmp -UseBasicParsing
    Move-Item -Force $Tmp $Dest
}

# 1. PDFium (strictly non-V8 Windows x64)
$PdfiumArchive = Join-Path $CacheDir "pdfium-win-x64.tgz"
Download-Cached "$PdfiumBaseUrl/pdfium-win-x64.tgz" $PdfiumArchive
$PdfiumExtract = Join-Path $CacheDir "pdfium_win"
if (Test-Path $PdfiumExtract) { Remove-Item -Recurse -Force $PdfiumExtract }
New-Item -ItemType Directory -Force -Path $PdfiumExtract | Out-Null
& tar -xzf $PdfiumArchive -C $PdfiumExtract
$PdfiumDll = Join-Path $PdfiumExtract "bin\pdfium.dll"
if (-not (Test-Path $PdfiumDll)) {
    throw "pdfium.dll not found inside pdfium-win-x64.tgz"
}
Copy-Item $PdfiumDll (Join-Path $ResourcesDir "pdfium.dll")
$PdfiumLic = Join-Path $PdfiumExtract "LICENSE"
if (Test-Path $PdfiumLic) {
    Copy-Item $PdfiumLic (Join-Path $LicensesOut "PDFium-LICENSE")
}

# 2. ONNX Runtime (CPU build ONLY; verify DirectML.dll is NOT staged)
$OrtArchive = Join-Path $CacheDir "onnxruntime-win-x64-$OnnxRuntimeVersion.zip"
Download-Cached "https://github.com/microsoft/onnxruntime/releases/download/v$OnnxRuntimeVersion/onnxruntime-win-x64-$OnnxRuntimeVersion.zip" $OrtArchive
$OrtExtract = Join-Path $CacheDir "ort_win"
if (Test-Path $OrtExtract) { Remove-Item -Recurse -Force $OrtExtract }
Expand-Archive -Path $OrtArchive -DestinationPath $OrtExtract -Force
$OrtRoot = Join-Path $OrtExtract "onnxruntime-win-x64-$OnnxRuntimeVersion"
$OrtDll = Join-Path $OrtRoot "lib\onnxruntime.dll"
if (-not (Test-Path $OrtDll)) {
    throw "onnxruntime.dll not found at $OrtDll"
}
Copy-Item $OrtDll (Join-Path $ResourcesDir "onnxruntime.dll")
$OrtLic = Join-Path $OrtRoot "LICENSE"
if (Test-Path $OrtLic) {
    Copy-Item $OrtLic (Join-Path $LicensesOut "ONNXRuntime-LICENSE")
}

if (Test-Path (Join-Path $ResourcesDir "DirectML.dll")) {
    throw "DirectML.dll must never be bundled (would exceed 40 MB budget)"
}

# 3. DjVuLibre standalone executables (ddjvu.exe, djvused.exe, djvutxt.exe) + non-system runtime DLLs
$SearchDirs = @()
if ($DjvuBinDir) { $SearchDirs += $DjvuBinDir }
$DdjvuOnPath = Get-Command ddjvu.exe -ErrorAction SilentlyContinue
if ($DdjvuOnPath) { $SearchDirs += (Split-Path -Parent $DdjvuOnPath.Source) }
if ($env:MSYS2_LOCATION) {
    $SearchDirs += @(
        (Join-Path $env:MSYS2_LOCATION "ucrt64\bin"),
        (Join-Path $env:MSYS2_LOCATION "mingw64\bin")
    )
}
if ($env:RUNNER_TEMP) {
    $SearchDirs += @(
        (Join-Path $env:RUNNER_TEMP "msys64\ucrt64\bin"),
        (Join-Path $env:RUNNER_TEMP "msys64\mingw64\bin")
    )
}
$SearchDirs += @(
    "C:\msys64\ucrt64\bin",
    "C:\msys64\mingw64\bin",
    "D:\msys64\ucrt64\bin",
    "D:\a\_temp\msys64\ucrt64\bin",
    "${env:ProgramFiles}\DjVuLibre",
    "${env:ProgramFiles(x86)}\DjVuLibre"
)

$FoundDjvuDir = $null
foreach ($dir in $SearchDirs) {
    if ($dir -and (Test-Path (Join-Path $dir "ddjvu.exe"))) {
        $FoundDjvuDir = $dir
        break
    }
}

if (-not $FoundDjvuDir) {
    throw "DjVuLibre tools (ddjvu.exe) not found. Install MSYS2 mingw-w64-ucrt-x86_64-djvulibre or set DJVULIBRE_BIN_DIR."
}

Write-Host "    Using DjVuLibre binaries from: $FoundDjvuDir"
foreach ($exe in @("ddjvu.exe", "djvused.exe", "djvutxt.exe")) {
    $src = Join-Path $FoundDjvuDir $exe
    if (-not (Test-Path $src)) {
        throw "Required DjVuLibre tool missing: $src"
    }
    Copy-Item $src (Join-Path $ResourcesDir $exe)
}

# Inspect PE import tables via Python (or ldd fallback) to copy only required DLLs from $FoundDjvuDir
$PyCmd = Get-Command python -ErrorAction SilentlyContinue
if (-not $PyCmd) { $PyCmd = Get-Command python3 -ErrorAction SilentlyContinue }
if ($PyCmd) {
    $PyScript = @'
import os, shutil, struct, sys

res_dir = sys.argv[1]
bin_dir = sys.argv[2]

def pe_imports(path):
    with open(path, "rb") as f:
        data = f.read()
    if data[:2] != b"MZ":
        return []
    pe_off = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe_off:pe_off+4] != b"PE\0\0":
        return []
    coff_off = pe_off + 4
    num_sec = struct.unpack_from("<H", data, coff_off + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff_off + 16)[0]
    opt_off = coff_off + 20
    magic = struct.unpack_from("<H", data, opt_off)[0]
    # PE32+: data dirs at opt_off + 112; PE32: opt_off + 96
    dd_off = opt_off + (112 if magic == 0x20B else 96)
    imp_rva, imp_size = struct.unpack_from("<II", data, dd_off + 8)
    if imp_rva == 0:
        return []
    sec_off = opt_off + opt_size
    sections = []
    for i in range(num_sec):
        s = sec_off + i * 40
        vsize, va, rsize, rptr = struct.unpack_from("<IIII", data, s + 8)
        sections.append((va, max(vsize, rsize), rptr))
    def rva_to_off(rva):
        for va, span, rptr in sections:
            if va <= rva < va + span:
                return rptr + (rva - va)
        return None
    off = rva_to_off(imp_rva)
    if off is None:
        return []
    dlls = []
    while off + 20 <= len(data):
        ilt, ts, fc, name_rva, ft = struct.unpack_from("<IIIII", data, off)
        if name_rva == 0 and ilt == 0 and ft == 0:
            break
        noff = rva_to_off(name_rva)
        if noff is not None:
            end = data.find(b"\0", noff)
            if end != -1:
                dlls.append(data[noff:end].decode("ascii", errors="ignore"))
        off += 20
    return dlls

dir_map = {fn.lower(): os.path.join(bin_dir, fn) for fn in os.listdir(bin_dir) if fn.lower().endswith(".dll")}
queue = [os.path.join(res_dir, x) for x in ("ddjvu.exe", "djvused.exe", "djvutxt.exe")]
seen = set()
copied = []
while queue:
    item = queue.pop(0)
    if item in seen:
        continue
    seen.add(item)
    for dll in pe_imports(item):
        low = dll.lower()
        if low in dir_map:
            dst = os.path.join(res_dir, os.path.basename(dir_map[low]))
            if not os.path.exists(dst):
                shutil.copy2(dir_map[low], dst)
                copied.append(dst)
                queue.append(dst)
print(f"    Staged {len(copied)} DjVuLibre runtime DLL(s) from {bin_dir}.")
'@
    & $PyCmd.Source -c $PyScript $ResourcesDir $FoundDjvuDir
    if ($LASTEXITCODE -ne 0) { throw "PE dependency resolution failed" }
} else {
    # Fallback pattern copy when Python is unavailable
    $Patterns = @("libdjvulibre*.dll", "libjpeg*.dll", "libtiff*.dll", "libwebp*.dll", "libsharpyuv*.dll", "libzstd*.dll", "liblzma*.dll", "libdeflate*.dll", "libLerc*.dll", "libjbig*.dll", "zlib1.dll", "libgcc_s_*.dll", "libstdc++-*.dll", "libwinpthread-*.dll")
    foreach ($pat in $Patterns) {
        Get-ChildItem -Path $FoundDjvuDir -Filter $pat -ErrorAction SilentlyContinue | ForEach-Object {
            Copy-Item $_.FullName (Join-Path $ResourcesDir $_.Name)
        }
    }
}

Write-Host "==> Staged Windows resources summary ($ResourcesDir):"
Get-ChildItem -Path $ResourcesDir | Format-Table Name, Length -AutoSize
