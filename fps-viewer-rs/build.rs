//! Compiles `assets/app.rc` (icon, UAC/DPI manifest, version info) into a `.res` file and
//! hands it to the linker. Works from Windows (rc.exe / llvm-rc) and when cross-compiling
//! from macOS or Linux (llvm-rc).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-env-changed=RC");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest_dir.join("assets");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out = out_dir.join("app.res");

    // `assets/app.rc` is a template: the version comes from Cargo.toml, so the exe's
    // Properties → Details always match the crate version. Bump it there, never in app.rc.
    // (Version info shows in Explorer, Task Manager and the UAC prompt; keep CompanyName
    // matching the code-signing certificate's subject.)
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let num = |k: &str| env::var(k).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let version_4 = format!(
        "{},{},{},0",
        num("CARGO_PKG_VERSION_MAJOR"),
        num("CARGO_PKG_VERSION_MINOR"),
        num("CARGO_PKG_VERSION_PATCH")
    );
    let template = std::fs::read_to_string(assets.join("app.rc")).expect("read assets/app.rc");
    let rc_src = out_dir.join("app.rc");
    std::fs::write(&rc_src, template.replace("{VERSION_4}", &version_4).replace("{VERSION}", &version))
        .expect("write generated app.rc");

    let rc = find_rc().unwrap_or_else(|| {
        panic!(
            "No resource compiler found. Install LLVM (llvm-rc) or the Windows SDK (rc.exe), \
             or set the RC environment variable to its path."
        )
    });

    let mut cmd = Command::new(&rc);
    let is_llvm = rc.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("llvm-rc"));
    // '-' switches (not '/') so absolute macOS/Linux paths aren't mistaken for options;
    // both rc.exe and llvm-rc accept them.
    if is_llvm {
        cmd.arg("-no-preprocess");
    }
    // Run from OUT_DIR with the input as a bare relative name — an absolute "/Users/…" input
    // would be parsed as the /U switch — and assets/ on the include path for app.ico/.manifest.
    cmd.arg("-nologo").arg("-I").arg(&assets).arg("-fo").arg(&out).arg("app.rc").current_dir(&out_dir);

    let status = cmd.status().unwrap_or_else(|e| panic!("failed to run {}: {e}", rc.display()));
    assert!(status.success(), "{} failed to compile app.rc", rc.display());

    println!("cargo:rustc-link-arg-bins={}", out.display());
}

fn find_rc() -> Option<PathBuf> {
    if let Ok(rc) = env::var("RC") {
        return Some(PathBuf::from(rc));
    }

    let mut candidates: Vec<PathBuf> = vec![
        "/opt/homebrew/opt/llvm/bin/llvm-rc".into(),
        "/usr/local/opt/llvm/bin/llvm-rc".into(),
        "/usr/bin/llvm-rc".into(),
    ];
    for dir in env::var_os("PATH").map(|p| env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default() {
        candidates.push(dir.join("llvm-rc"));
        candidates.push(dir.join("llvm-rc.exe"));
        candidates.push(dir.join("rc.exe"));
    }
    if cfg!(windows) {
        candidates.push(r"C:\Program Files\LLVM\bin\llvm-rc.exe".into());
        candidates.extend(windows_sdk_rc());
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Newest `rc.exe` from an installed Windows 10/11 SDK.
fn windows_sdk_rc() -> Vec<PathBuf> {
    let root = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let host = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let mut versions: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(host).join("rc.exe")).collect()
}
