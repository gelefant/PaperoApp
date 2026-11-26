use crate::config::LinuxRevealMode;
use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

pub const MACOS_PREVIEW_PRESET_ID: &str = "macos-preview-applescript";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalReaderPreset {
    pub id: String,
    pub label: String,
    pub os: String,
    pub formats: Vec<String>,
    pub template: String,
    pub requires_accessibility: bool,
    pub note: Option<String>,
}

/// Returns verified external reader presets (with `{file}` and `{page}` placeholders).
pub fn external_reader_presets() -> Vec<ExternalReaderPreset> {
    vec![
        ExternalReaderPreset {
            id: "djview".into(),
            label: "DjView (DjVu)".into(),
            os: "all".into(),
            formats: vec!["djvu".into()],
            template: "djview -pageno={page} {file}".into(),
            requires_accessibility: false,
            note: Some("-pageno=N usa sempre il numero di pagina ordinale.".into()),
        },
        ExternalReaderPreset {
            id: MACOS_PREVIEW_PRESET_ID.into(),
            label: "Anteprima (macOS AppleScript ⌥⌘G)".into(),
            os: "macos".into(),
            formats: vec!["pdf".into()],
            template: MACOS_PREVIEW_PRESET_ID.into(),
            requires_accessibility: true,
            note: Some(
                "Richiede il permesso di Accessibilità in Impostazioni di Sistema per inviare ⌥⌘G."
                    .into(),
            ),
        },
        ExternalReaderPreset {
            id: "skim".into(),
            label: "Skim (macOS)".into(),
            os: "macos".into(),
            formats: vec!["pdf".into()],
            template: "/Applications/Skim.app/Contents/SharedSupport/displayline {page} {file}"
                .into(),
            requires_accessibility: false,
            note: None,
        },
        ExternalReaderPreset {
            id: "sumatra".into(),
            label: "SumatraPDF (Windows)".into(),
            os: "windows".into(),
            formats: vec!["pdf".into(), "djvu".into()],
            template: "SumatraPDF.exe -page {page} {file}".into(),
            requires_accessibility: false,
            note: None,
        },
        ExternalReaderPreset {
            id: "okular".into(),
            label: "Okular (Linux / Windows)".into(),
            os: "all".into(),
            formats: vec!["pdf".into(), "djvu".into()],
            template: "okular -p {page} {file}".into(),
            requires_accessibility: false,
            note: None,
        },
        ExternalReaderPreset {
            id: "evince".into(),
            label: "Evince (Linux)".into(),
            os: "linux".into(),
            formats: vec!["pdf".into(), "djvu".into()],
            template: "evince -i {page} {file}".into(),
            requires_accessibility: false,
            note: None,
        },
        ExternalReaderPreset {
            id: "zathura".into(),
            label: "Zathura (Linux)".into(),
            os: "linux".into(),
            formats: vec!["pdf".into(), "djvu".into()],
            template: "zathura -P {page} {file}".into(),
            requires_accessibility: false,
            note: None,
        },
    ]
}

/// Opens `file_path` using either a configured `{file}`/`{page}` command template
/// or the OS default application. Never passes `{file}` through a shell (`sh -c` / `cmd /C`).
pub fn open_external(
    file_path: &Path,
    page_ord: Option<u32>,
    template: Option<&str>,
) -> Result<()> {
    let page = page_ord.unwrap_or(1).max(1);
    if let Some(tpl) = template.map(str::trim).filter(|s| !s.is_empty()) {
        if tpl == MACOS_PREVIEW_PRESET_ID {
            return open_in_macos_preview(file_path, page);
        }
        return spawn_from_template(tpl, file_path, page);
    }

    open_with_os_default(file_path)
}

#[cfg(target_os = "windows")]
mod win_shell {
    use std::ffi::c_void;

    pub const COINIT_APARTMENTTHREADED: u32 = 0x2;
    pub const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
    pub const SW_SHOWNORMAL: i32 = 1;

    #[link(name = "ole32")]
    extern "system" {
        pub fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> i32;
        pub fn CoUninitialize();
    }

    #[link(name = "shell32")]
    extern "system" {
        pub fn ILCreateFromPathW(psz_path: *const u16) -> *mut c_void;
        pub fn ILFree(pidl: *mut c_void);
        pub fn SHOpenFolderAndSelectItems(
            pidl_folder: *const c_void,
            cidl: u32,
            apidl: *const *const c_void,
            dw_flags: u32,
        ) -> i32;
        pub fn ShellExecuteW(
            hwnd: *mut c_void,
            lp_operation: *const u16,
            lp_file: *const u16,
            lp_parameters: *const u16,
            lp_directory: *const u16,
            n_show_cmd: i32,
        ) -> isize;
    }
}

/// Normalizes a filesystem path for Windows Shell APIs and `explorer.exe`
/// (converts `/` to `\` and strips `\\?\` / `\\?\UNC\` verbatim prefixes).
pub fn normalize_windows_shell_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('/', "\\");
    if let Some(unc_tail) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc_tail}")
    } else if let Some(drive_tail) = raw.strip_prefix(r"\\?\") {
        drive_tail.to_string()
    } else {
        raw
    }
}

/// Formats the raw command-line switch for `explorer.exe` so quotes wrap only the path
/// after `/select,` (avoiding Rust `Command::arg` wrapping quotes before `/select,`).
pub fn format_explorer_select_raw_arg(clean_win_path: &str) -> String {
    let sanitized = clean_win_path.replace('"', "");
    format!("/select,\"{sanitized}\"")
}

pub fn open_with_os_default(file_path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(file_path).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        let raw_lossy = file_path.to_string_lossy();
        let target = if raw_lossy.starts_with("https://") || raw_lossy.starts_with("http://") {
            raw_lossy.into_owned()
        } else {
            normalize_windows_shell_path(file_path)
        };
        let wide_open: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let wide_target: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: Both `wide_open` and `wide_target` are valid NUL-terminated UTF-16 buffers.
        let ret = unsafe {
            win_shell::ShellExecuteW(
                std::ptr::null_mut(),
                wide_open.as_ptr(),
                wide_target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                win_shell::SW_SHOWNORMAL,
            )
        };
        if ret <= 32 {
            let os_err = std::io::Error::last_os_error();
            return Err(CoreError::Io(std::io::Error::other(format!(
                "ShellExecuteW failed (code {ret}) for '{target}': {os_err}"
            ))));
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(file_path).spawn()?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Ok(())
}

/// Splits a command template into argv tokens (respecting single/double quotes)
/// and substitutes `{file}` and `{page}` directly as OS `Command::arg` arguments.
pub fn build_template_argv(template: &str, file_path: &Path, page: u32) -> Result<Vec<String>> {
    let tokens = split_shell_like_tokens(template)?;
    if tokens.is_empty() {
        return Err(CoreError::Extract("empty external command template".into()));
    }
    let file_str = file_path.to_string_lossy();
    let page_str = page.to_string();

    Ok(tokens
        .into_iter()
        .map(|tok| tok.replace("{file}", &file_str).replace("{page}", &page_str))
        .collect())
}

fn spawn_from_template(template: &str, file_path: &Path, page: u32) -> Result<()> {
    let argv = build_template_argv(template, file_path, page)?;
    let (prog, args) = (&argv[0], &argv[1..]);
    Command::new(prog).args(args).spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CoreError::MissingExternalTool {
                tool: prog.clone(),
                detail: format!("external reader '{prog}' not found: {e}"),
            }
        } else {
            CoreError::Io(e)
        }
    })?;
    Ok(())
}

fn open_in_macos_preview(file_path: &Path, page: u32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Pass file path and page number via AppleScript `on run argv` so filenames are never interpolated into script code.
        let script = r#"
on run argv
    set targetFile to POSIX file (item 1 of argv)
    set targetPage to (item 2 of argv) as text
    tell application "Preview"
        activate
        open targetFile
    end tell
    delay 0.4
    tell application "System Events"
        tell process "Preview"
            keystroke "g" using {option down, command down}
            delay 0.15
            keystroke targetPage
            key code 36
        end tell
    end tell
end run
"#;
        Command::new("osascript")
            .arg("-e")
            .arg(script)
            .arg("--")
            .arg(file_path)
            .arg(page.to_string())
            .spawn()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = page;
        open_with_os_default(file_path)
    }
}

/// Reveals `file_path` in the OS file manager (`open -R`, `explorer /select,`, or Linux D-Bus/xdg-open).
pub fn reveal_in_folder(file_path: &Path, linux_mode: LinuxRevealMode) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = linux_mode;
        Command::new("open").arg("-R").arg(file_path).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        let _ = linux_mode;
        let clean_win_path = normalize_windows_shell_path(file_path);
        let wide_path: Vec<u16> = clean_win_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // Primary path: native Win32 Shell `SHOpenFolderAndSelectItems` via PIDL.
        // Avoids `explorer.exe`'s custom command-line parser (which rejects Rust's outer quotes
        // around `"/select,..."` for paths with spaces and splits unquoted paths on commas `,` or `=`).
        // SAFETY: `wide_path` is NUL-terminated; `pidl` is freed via `ILFree` before returning,
        // and `CoUninitialize` is balanced only when `CoInitializeEx` returned `S_OK` / `S_FALSE` (`>= 0`).
        let shell_ok = unsafe {
            let hr_init = win_shell::CoInitializeEx(
                std::ptr::null_mut(),
                win_shell::COINIT_APARTMENTTHREADED | win_shell::COINIT_DISABLE_OLE1DDE,
            );
            let should_uninit = hr_init >= 0;
            let pidl = win_shell::ILCreateFromPathW(wide_path.as_ptr());
            let ok = if !pidl.is_null() {
                let hr = win_shell::SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0);
                win_shell::ILFree(pidl);
                hr >= 0
            } else {
                false
            };
            if should_uninit {
                win_shell::CoUninitialize();
            }
            ok
        };
        if shell_ok {
            return Ok(());
        }

        // Fallback: `explorer.exe` with `raw_arg` so quotes wrap ONLY the path after `/select,`.
        use std::os::windows::process::CommandExt;
        Command::new("explorer.exe")
            .raw_arg(format_explorer_select_raw_arg(&clean_win_path))
            .spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let parent = file_path.parent().unwrap_or(Path::new("/"));
        if linux_mode == LinuxRevealMode::DbusWithFallback {
            if let Ok(abs) = std::fs::canonicalize(file_path) {
                let encoded = percent_encoding::utf8_percent_encode(
                    &abs.to_string_lossy(),
                    percent_encoding::CONTROLS,
                )
                .to_string()
                .replace(' ', "%20");
                let uri = format!("file://{encoded}");
                let arg = format!("[\"{uri}\"]");
                let status = Command::new("gdbus")
                    .args([
                        "call",
                        "--session",
                        "--timeout",
                        "2",
                        "--dest",
                        "org.freedesktop.FileManager1",
                        "--object-path",
                        "/org/freedesktop/FileManager1",
                        "--method",
                        "org.freedesktop.FileManager1.ShowItems",
                        &arg,
                        "",
                    ])
                    .status();
                if matches!(status, Ok(s) if s.success()) {
                    return Ok(());
                }
            }
        }
        Command::new("xdg-open").arg(parent).spawn()?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Ok(())
}

fn split_shell_like_tokens(input: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if matches!(chars.peek(), Some('"')) {
                        current.push(chars.next().unwrap());
                    } else {
                        current.push('\\');
                    }
                } else {
                    current.push(c);
                }
            }
            None => {
                if c.is_whitespace() {
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                } else if c == '\'' || c == '"' {
                    quote = Some(c);
                    in_token = true;
                } else {
                    current.push(c);
                    in_token = true;
                }
            }
        }
    }

    if quote.is_some() {
        return Err(CoreError::Extract(
            "unterminated quote in external command template".into(),
        ));
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_argv_keeps_special_filenames_single_arg() {
        let path = Path::new("/tmp/books/Erdős $(rm -rf) \"paper\".pdf");
        let argv = build_template_argv("okular -p {page} {file}", path, 42).unwrap();
        assert_eq!(argv.len(), 4);
        assert_eq!(argv[0], "okular");
        assert_eq!(argv[1], "-p");
        assert_eq!(argv[2], "42");
        assert_eq!(argv[3], "/tmp/books/Erdős $(rm -rf) \"paper\".pdf");
    }

    #[test]
    fn template_supports_quoted_program_paths() {
        let path = Path::new("C:\\books\\a.pdf");
        let argv = build_template_argv(
            "\"C:\\Program Files\\SumatraPDF\\SumatraPDF.exe\" -page {page} {file}",
            path,
            7,
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "C:\\Program Files\\SumatraPDF\\SumatraPDF.exe",
                "-page",
                "7",
                "C:\\books\\a.pdf"
            ]
        );
    }

    #[test]
    fn windows_shell_path_strips_verbatim_prefixes_and_formats_explorer_select() {
        assert_eq!(
            normalize_windows_shell_path(Path::new(r"\\?\C:\Users\Mario Rossi\Libri\Serre, J.-P.djvu")),
            r"C:\Users\Mario Rossi\Libri\Serre, J.-P.djvu"
        );
        assert_eq!(
            normalize_windows_shell_path(Path::new(r"\\?\UNC\nas\math\Grothendieck EGA 1.djvu")),
            r"\\nas\math\Grothendieck EGA 1.djvu"
        );
        assert_eq!(
            normalize_windows_shell_path(Path::new("D:/Algebraic Geometry/Hartshorne, R.pdf")),
            r"D:\Algebraic Geometry\Hartshorne, R.pdf"
        );
        assert_eq!(
            format_explorer_select_raw_arg(r"C:\Users\Mario Rossi\Libri\Serre, J.-P.djvu"),
            "/select,\"C:\\Users\\Mario Rossi\\Libri\\Serre, J.-P.djvu\""
        );
    }
}
