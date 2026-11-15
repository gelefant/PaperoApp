#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RESOURCES_DIR="${REPO_ROOT}/src-tauri/resources"
CACHE_DIR="${REPO_ROOT}/.tmp_stage_cache"

ONNXRUNTIME_VERSION="${ONNXRUNTIME_VERSION:-1.23.2}"
PDFIUM_BASE_URL="${PDFIUM_BASE_URL:-https://github.com/bblanchon/pdfium-binaries/releases/latest/download}"

TARGET=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      TARGET="$2"
      shift 2
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "${TARGET}" ]]; then
  OS_NAME="$(uname -s)"
  ARCH_NAME="$(uname -m)"
  case "${OS_NAME}:${ARCH_NAME}" in
    Darwin:arm64) TARGET="aarch64-apple-darwin" ;;
    Darwin:x86_64) TARGET="x86_64-apple-darwin" ;;
    Linux:x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
    *)
      echo "Unsupported host OS/arch ${OS_NAME}:${ARCH_NAME}; pass --target explicitly" >&2
      exit 1
      ;;
  esac
fi

echo "==> Staging Papero native resources for target: ${TARGET}"

mkdir -p "${CACHE_DIR}" "${RESOURCES_DIR}"
# Clean previous staged binaries while preserving .gitkeep
find "${RESOURCES_DIR}" -mindepth 1 ! -name '.gitkeep' -delete
mkdir -p "${RESOURCES_DIR}/licenses"

# Always copy Papero / DjVuLibre compliance files
cp "${REPO_ROOT}/LICENSE-MIT" "${RESOURCES_DIR}/licenses/LICENSE-MIT"
cp "${REPO_ROOT}/LICENSE-APACHE" "${RESOURCES_DIR}/licenses/LICENSE-APACHE"
cp "${REPO_ROOT}/THIRD_PARTY_LICENSES.md" "${RESOURCES_DIR}/licenses/THIRD_PARTY_LICENSES.md"
cp "${REPO_ROOT}/licenses/GPL-2.0-DjVuLibre.txt" "${RESOURCES_DIR}/licenses/GPL-2.0-DjVuLibre.txt"
cp "${REPO_ROOT}/licenses/SOURCE_OFFER.txt" "${RESOURCES_DIR}/licenses/SOURCE_OFFER.txt"

download_file() {
  local url="$1"
  local dest="$2"
  if [[ -s "${dest}" ]]; then
    echo "    [cache hit] $(basename "${dest}")"
    return 0
  fi
  echo "    [download]  ${url}"
  curl -fL --retry 3 --retry-delay 2 -o "${dest}.part" "${url}"
  mv "${dest}.part" "${dest}"
}

relocate_macos_deps() {
  local expected_arch="$1"
  local search_roots="$2"
  python3 - "${RESOURCES_DIR}" "${expected_arch}" "${search_roots}" <<'PY'
import os
import re
import shutil
import subprocess
import sys

res_dir = os.path.abspath(sys.argv[1])
expected_arch = sys.argv[2]  # arm64 or x86_64
search_roots = [r for r in sys.argv[3].split(":") if r and os.path.isdir(r)]

# Build basename -> real path index inside search_roots
lib_index = {}
for root in search_roots:
    for cur, _, files in os.walk(root):
        for fn in files:
            if fn.endswith(".dylib"):
                p = os.path.realpath(os.path.join(cur, fn))
                lib_index.setdefault(fn, p)

def otool_deps(path: str) -> list[str]:
    out = subprocess.check_output(["otool", "-L", path], text=True)
    deps = []
    for line in out.splitlines()[1:]:
        m = re.match(r"^\s*(\S+)\s+\(", line)
        if m:
            deps.append(m.group(1))
    return deps

def is_system_dep(dep: str) -> bool:
    return dep.startswith("/System/") or dep.startswith("/usr/lib/")

def resolve_dep(dep: str, current_file: str) -> str | None:
    if dep.startswith("@loader_path/"):
        cand = os.path.join(os.path.dirname(current_file), dep[len("@loader_path/"):])
        if os.path.exists(cand):
            return os.path.realpath(cand)
    # Exact path if non-placeholder exists on disk matching target arch
    if os.path.exists(dep) and not dep.startswith("@@"):
        return os.path.realpath(dep)
    # Match basename against extracted target search roots (handles @rpath/, @@HOMEBREW_PREFIX@@/, /opt/homebrew/)
    base = os.path.basename(dep)
    if base in lib_index:
        return lib_index[base]
    return None

bins = [os.path.join(res_dir, b) for b in ("ddjvu", "djvused", "djvutxt")]
queue = list(bins)
seen = set()
copied = []

while queue:
    item = queue.pop(0)
    if item in seen:
        continue
    seen.add(item)
    for dep in otool_deps(item):
        if is_system_dep(dep) or dep.startswith("@loader_path/"):
            # Even if @loader_path/, ensure target exists in res_dir
            base = os.path.basename(dep)
            dst = os.path.join(res_dir, base)
            if dep.startswith("@loader_path/") and not os.path.exists(dst):
                src = resolve_dep(dep, item)
                if src:
                    shutil.copy2(src, dst)
                    os.chmod(dst, 0o755)
                    copied.append(dst)
                    queue.append(dst)
            continue
        base = os.path.basename(dep)
        dst = os.path.join(res_dir, base)
        if not os.path.exists(dst):
            src = resolve_dep(dep, item)
            if not src:
                raise RuntimeError(f"Could not resolve non-system dylib '{dep}' referenced by '{item}'")
            shutil.copy2(src, dst)
            os.chmod(dst, 0o755)
            copied.append(dst)
            queue.append(dst)
        elif dst not in seen:
            queue.append(dst)

# Rewrite install_name IDs & references to @loader_path/<basename>
all_macho = bins + copied
for path in all_macho:
    os.chmod(path, 0o755)
    base = os.path.basename(path)
    if base.endswith(".dylib"):
        subprocess.run(["install_name_tool", "-id", f"@loader_path/{base}", path], check=True)
    for dep in otool_deps(path):
        if is_system_dep(dep):
            continue
        dep_base = os.path.basename(dep)
        if dep_base == base:
            continue
        if dep != f"@loader_path/{dep_base}":
            subprocess.run(["install_name_tool", "-change", dep, f"@loader_path/{dep_base}", path], check=True)

# Verify target architecture and ad-hoc sign
for path in all_macho + [os.path.join(res_dir, "libpdfium.dylib"), os.path.join(res_dir, "libonnxruntime.dylib")]:
    if os.path.exists(path):
        info = subprocess.check_output(["file", path], text=True)
        if expected_arch not in info:
            raise RuntimeError(f"Architecture mismatch for {path}: expected {expected_arch}, got: {info.strip()}")
        subprocess.run(["codesign", "--force", "--sign", "-", path], check=True)

print(f"    Relocated {len(copied)} transitive dylib(s) cleanly for {expected_arch}.")
PY
}

case "${TARGET}" in
  aarch64-apple-darwin)
    PDFIUM_ARCHIVE="${CACHE_DIR}/pdfium-mac-arm64.tgz"
    ORT_ARCHIVE="${CACHE_DIR}/onnxruntime-osx-arm64-${ONNXRUNTIME_VERSION}.tgz"

    download_file "${PDFIUM_BASE_URL}/pdfium-mac-arm64.tgz" "${PDFIUM_ARCHIVE}"
    tar -xzf "${PDFIUM_ARCHIVE}" -C "${CACHE_DIR}" ./lib/libpdfium.dylib ./LICENSE 2>/dev/null || \
      tar -xzf "${PDFIUM_ARCHIVE}" -C "${CACHE_DIR}" lib/libpdfium.dylib LICENSE
    cp "${CACHE_DIR}/lib/libpdfium.dylib" "${RESOURCES_DIR}/libpdfium.dylib"
    cp "${CACHE_DIR}/LICENSE" "${RESOURCES_DIR}/licenses/PDFium-LICENSE"
    install_name_tool -id "@rpath/libpdfium.dylib" "${RESOURCES_DIR}/libpdfium.dylib"

    download_file "https://github.com/microsoft/onnxruntime/releases/download/v${ONNXRUNTIME_VERSION}/onnxruntime-osx-arm64-${ONNXRUNTIME_VERSION}.tgz" "${ORT_ARCHIVE}"
    tar -xzf "${ORT_ARCHIVE}" -C "${CACHE_DIR}" "onnxruntime-osx-arm64-${ONNXRUNTIME_VERSION}/lib/libonnxruntime.${ONNXRUNTIME_VERSION}.dylib" "onnxruntime-osx-arm64-${ONNXRUNTIME_VERSION}/LICENSE"
    cp "${CACHE_DIR}/onnxruntime-osx-arm64-${ONNXRUNTIME_VERSION}/lib/libonnxruntime.${ONNXRUNTIME_VERSION}.dylib" "${RESOURCES_DIR}/libonnxruntime.dylib"
    cp "${CACHE_DIR}/onnxruntime-osx-arm64-${ONNXRUNTIME_VERSION}/LICENSE" "${RESOURCES_DIR}/licenses/ONNXRuntime-LICENSE"
    install_name_tool -id "@rpath/libonnxruntime.dylib" "${RESOURCES_DIR}/libonnxruntime.dylib"

    DJVU_PREFIX="$(brew --prefix djvulibre 2>/dev/null || echo "/opt/homebrew/opt/djvulibre")"
    if [[ ! -x "${DJVU_PREFIX}/bin/ddjvu" ]]; then
      echo "ERROR: DjVuLibre not found at ${DJVU_PREFIX}. Run 'brew install djvulibre' first." >&2
      exit 1
    fi
    cp "${DJVU_PREFIX}/bin/ddjvu" "${RESOURCES_DIR}/ddjvu"
    cp "${DJVU_PREFIX}/bin/djvused" "${RESOURCES_DIR}/djvused"
    cp "${DJVU_PREFIX}/bin/djvutxt" "${RESOURCES_DIR}/djvutxt"

    relocate_macos_deps "arm64" "${DJVU_PREFIX}/lib:/opt/homebrew/opt:/opt/homebrew/Cellar"
    ;;

  x86_64-apple-darwin)
    PDFIUM_ARCHIVE="${CACHE_DIR}/pdfium-mac-x64.tgz"
    ORT_ARCHIVE="${CACHE_DIR}/onnxruntime-osx-x86_64-${ONNXRUNTIME_VERSION}.tgz"

    download_file "${PDFIUM_BASE_URL}/pdfium-mac-x64.tgz" "${PDFIUM_ARCHIVE}"
    tar -xzf "${PDFIUM_ARCHIVE}" -C "${CACHE_DIR}" ./lib/libpdfium.dylib ./LICENSE 2>/dev/null || \
      tar -xzf "${PDFIUM_ARCHIVE}" -C "${CACHE_DIR}" lib/libpdfium.dylib LICENSE
    cp "${CACHE_DIR}/lib/libpdfium.dylib" "${RESOURCES_DIR}/libpdfium.dylib"
    cp "${CACHE_DIR}/LICENSE" "${RESOURCES_DIR}/licenses/PDFium-LICENSE"
    install_name_tool -id "@rpath/libpdfium.dylib" "${RESOURCES_DIR}/libpdfium.dylib"

    download_file "https://github.com/microsoft/onnxruntime/releases/download/v${ONNXRUNTIME_VERSION}/onnxruntime-osx-x86_64-${ONNXRUNTIME_VERSION}.tgz" "${ORT_ARCHIVE}"
    tar -xzf "${ORT_ARCHIVE}" -C "${CACHE_DIR}" "onnxruntime-osx-x86_64-${ONNXRUNTIME_VERSION}/lib/libonnxruntime.${ONNXRUNTIME_VERSION}.dylib" "onnxruntime-osx-x86_64-${ONNXRUNTIME_VERSION}/LICENSE"
    cp "${CACHE_DIR}/onnxruntime-osx-x86_64-${ONNXRUNTIME_VERSION}/lib/libonnxruntime.${ONNXRUNTIME_VERSION}.dylib" "${RESOURCES_DIR}/libonnxruntime.dylib"
    cp "${CACHE_DIR}/onnxruntime-osx-x86_64-${ONNXRUNTIME_VERSION}/LICENSE" "${RESOURCES_DIR}/licenses/ONNXRuntime-LICENSE"
    install_name_tool -id "@rpath/libonnxruntime.dylib" "${RESOURCES_DIR}/libonnxruntime.dylib"

    if [[ -x "/usr/local/opt/djvulibre/bin/ddjvu" ]] && file "/usr/local/opt/djvulibre/bin/ddjvu" | grep -q "x86_64"; then
      cp /usr/local/opt/djvulibre/bin/{ddjvu,djvused,djvutxt} "${RESOURCES_DIR}/"
      relocate_macos_deps "x86_64" "/usr/local/opt:/usr/local/Cellar"
    else
      DJVULIBRE_VERSION="${DJVULIBRE_VERSION:-3.5.30}"
      DJVULIBRE_URL="${DJVULIBRE_URL:-https://downloads.sourceforge.net/project/djvu/DjVuLibre/${DJVULIBRE_VERSION}/djvulibre-${DJVULIBRE_VERSION}.tar.gz}"
      DJVULIBRE_SHA256="${DJVULIBRE_SHA256:-ee5e457d4cfebe566f94b99e5e3d3cc7f5c79ddb741c2ac2ba2e456f00329644}"
      DJVU_TARBALL="${CACHE_DIR}/djvulibre-${DJVULIBRE_VERSION}.tar.gz"
      X64_SRC="${CACHE_DIR}/djvulibre_x64_src"
      X64_PREFIX="${CACHE_DIR}/djvulibre_x64_prefix"

      echo "    Cross-compiling DjVuLibre ${DJVULIBRE_VERSION} for x86_64-apple-darwin..."
      download_file "${DJVULIBRE_URL}" "${DJVU_TARBALL}"
      echo "${DJVULIBRE_SHA256}  ${DJVU_TARBALL}" | shasum -a 256 -c -
      rm -rf "${X64_SRC}" "${X64_PREFIX}"
      mkdir -p "${X64_SRC}" "${X64_PREFIX}"
      tar -xzf "${DJVU_TARBALL}" -C "${X64_SRC}"
      (
        cd "${X64_SRC}/djvulibre-${DJVULIBRE_VERSION}"
        ./configure \
          --host=x86_64-apple-darwin \
          --prefix="${X64_PREFIX}" \
          --disable-desktopfiles \
          --disable-xmltools \
          --without-tiff \
          --without-jpeg \
          CC="clang -arch x86_64 -mmacosx-version-min=10.15" \
          CXX="clang++ -arch x86_64 -mmacosx-version-min=10.15" \
          CFLAGS="-O2" \
          CXXFLAGS="-O2"
        make -j"$(sysctl -n hw.ncpu)" install
      )
      cp "${X64_PREFIX}/bin/ddjvu" "${RESOURCES_DIR}/ddjvu"
      cp "${X64_PREFIX}/bin/djvused" "${RESOURCES_DIR}/djvused"
      cp "${X64_PREFIX}/bin/djvutxt" "${RESOURCES_DIR}/djvutxt"
      relocate_macos_deps "x86_64" "${X64_PREFIX}/lib"
    fi
    ;;

  x86_64-unknown-linux-gnu)
    PDFIUM_ARCHIVE="${CACHE_DIR}/pdfium-linux-x64.tgz"
    ORT_ARCHIVE="${CACHE_DIR}/onnxruntime-linux-x64-${ONNXRUNTIME_VERSION}.tgz"

    download_file "${PDFIUM_BASE_URL}/pdfium-linux-x64.tgz" "${PDFIUM_ARCHIVE}"
    tar -xzf "${PDFIUM_ARCHIVE}" -C "${CACHE_DIR}" ./lib/libpdfium.so ./LICENSE 2>/dev/null || \
      tar -xzf "${PDFIUM_ARCHIVE}" -C "${CACHE_DIR}" lib/libpdfium.so LICENSE
    cp "${CACHE_DIR}/lib/libpdfium.so" "${RESOURCES_DIR}/libpdfium.so"
    cp "${CACHE_DIR}/LICENSE" "${RESOURCES_DIR}/licenses/PDFium-LICENSE"

    download_file "https://github.com/microsoft/onnxruntime/releases/download/v${ONNXRUNTIME_VERSION}/onnxruntime-linux-x64-${ONNXRUNTIME_VERSION}.tgz" "${ORT_ARCHIVE}"
    tar -xzf "${ORT_ARCHIVE}" -C "${CACHE_DIR}" "onnxruntime-linux-x64-${ONNXRUNTIME_VERSION}/lib/libonnxruntime.so.${ONNXRUNTIME_VERSION}" "onnxruntime-linux-x64-${ONNXRUNTIME_VERSION}/LICENSE"
    cp -L "${CACHE_DIR}/onnxruntime-linux-x64-${ONNXRUNTIME_VERSION}/lib/libonnxruntime.so.${ONNXRUNTIME_VERSION}" "${RESOURCES_DIR}/libonnxruntime.so"
    cp "${CACHE_DIR}/onnxruntime-linux-x64-${ONNXRUNTIME_VERSION}/LICENSE" "${RESOURCES_DIR}/licenses/ONNXRuntime-LICENSE"

    for tool in ddjvu djvused djvutxt; do
      tool_path="$(command -v "${tool}" || true)"
      if [[ -z "${tool_path}" ]]; then
        echo "ERROR: '${tool}' not found. Install 'djvulibre-bin' first." >&2
        exit 1
      fi
      cp -L "${tool_path}" "${RESOURCES_DIR}/${tool}"
      chmod 755 "${RESOURCES_DIR}/${tool}"
    done
    # Bundle libdjvulibre.so.21 cleanly if present so AppImage is portable
    libdjvu_path="$(ldd "$(command -v ddjvu)" | awk '/libdjvulibre/{print $3; exit}' || true)"
    if [[ -n "${libdjvu_path}" && -f "${libdjvu_path}" ]]; then
      libdjvu_base="$(basename "${libdjvu_path}")"
      cp -L "${libdjvu_path}" "${RESOURCES_DIR}/${libdjvu_base}"
      if command -v patchelf >/dev/null 2>&1; then
        for tool in ddjvu djvused djvutxt; do
          patchelf --set-rpath '$ORIGIN' "${RESOURCES_DIR}/${tool}" || true
        done
      fi
    fi
    ;;

  *)
    echo "Unsupported target triple: ${TARGET}" >&2
    exit 1
    ;;
esac

echo "==> Staged resources summary (${RESOURCES_DIR}):"
du -sh "${RESOURCES_DIR}"
ls -lh "${RESOURCES_DIR}"
