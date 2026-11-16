const WINDOWS_COMMON_CONTROLS_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    // `tauri-build` normally embeds the Common Controls v6 manifest only into binary targets via
    // `rustc-link-arg-bins`, leaving Windows MSVC unit/integration test binaries without Comctl32 v6
    // so `Comctl32!TaskDialogIndirect` fails at process startup with `0xc0000139`
    // (`STATUS_ENTRYPOINT_NOT_FOUND`). Passing `/MANIFEST:EMBED` + `/MANIFESTINPUT` via
    // `rustc-link-arg` applies the same manifest cleanly to binaries and test executables alike.
    if target_os == "windows" && target_env == "msvc" {
        let out_dir = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts"),
        );
        let manifest_path = out_dir.join("papero-app-manifest.xml");
        std::fs::write(&manifest_path, WINDOWS_COMMON_CONTROLS_MANIFEST)
            .expect("failed to write Windows application manifest");

        let attrs = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        tauri_build::try_build(attrs).expect("failed to run tauri-build");

        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest_path.display());
    } else {
        tauri_build::build();
    }
}
