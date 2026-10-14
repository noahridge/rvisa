//! Finding and loading the vendor VISA library (ROADMAP D2, DESIGN.md V4).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::ffi::Library;

/// The environment variable that names the VISA library to load.
pub const LIBRARY_ENV: &str = "RVISA_VENDOR_LIB";

/// The libraries to try, in order: an explicit path, then the
/// [`LIBRARY_ENV`] variable, then the platform defaults.
#[must_use]
pub fn candidates(explicit: Option<&Path>, env: Option<OsString>) -> Vec<PathBuf> {
    let mut list = Vec::new();
    if let Some(path) = explicit {
        list.push(path.to_path_buf());
    }
    if let Some(path) = env.filter(|p| !p.is_empty()) {
        list.push(PathBuf::from(path));
    }
    list.extend(platform_defaults());
    let mut seen = std::collections::HashSet::new();
    list.retain(|p| seen.insert(p.clone()));
    list
}

#[cfg(windows)]
fn platform_defaults() -> Vec<PathBuf> {
    let name = if cfg!(target_pointer_width = "64") {
        "visa64.dll"
    } else {
        "visa32.dll"
    };
    // A bare name uses the normal DLL search path (System32 included).
    let mut list = vec![PathBuf::from(name)];
    for dir in registry_framework_dirs() {
        for sub in ["Win64\\Bin", "WinNT\\Bin"] {
            list.push(dir.join(sub).join(name));
        }
    }
    list
}

/// The VXIplug&play framework directories recorded in the registry
/// (DESIGN.md V4).
#[cfg(windows)]
fn registry_framework_dirs() -> Vec<PathBuf> {
    const KEY: &str = r"SOFTWARE\VXIPNP_Alliance\VXIPNP\CurrentVersion";
    let Ok(key) = windows_registry::LOCAL_MACHINE.open(KEY) else {
        return Vec::new();
    };
    ["VXIPNPPATH64", "VXIPNPPATH"]
        .into_iter()
        .filter_map(|value| key.get_string(value).ok())
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "macos")]
fn platform_defaults() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/Frameworks/VISA.framework/VISA"),
        PathBuf::from("/Library/Frameworks/RsVisa.framework/RsVisa"),
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_defaults() -> Vec<PathBuf> {
    // The dynamic loader's own search first, then versioned names in the
    // standard directories.
    let mut list = vec![PathBuf::from("libvisa.so")];
    let mut dirs = vec![
        PathBuf::from("/usr/local/lib"),
        PathBuf::from("/usr/lib64"),
        PathBuf::from("/usr/lib"),
    ];
    if let Some(arch) = multiarch_dir() {
        dirs.push(PathBuf::from("/usr/lib").join(arch));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == "libvisa.so" || n.starts_with("libvisa.so."))
            })
            .collect();
        found.sort();
        list.extend(found);
    }
    list
}

#[cfg(not(any(windows, target_os = "macos")))]
fn multiarch_dir() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        "arm" => Some("arm-linux-gnueabihf"),
        "x86" => Some("i386-linux-gnu"),
        _ => None,
    }
}

/// Loads the first candidate that is a VISA library. On failure, returns a
/// message listing every attempt and how to fix it.
pub(crate) fn load(explicit: Option<&Path>) -> Result<Library, String> {
    load_first(candidates(explicit, std::env::var_os(LIBRARY_ENV)))
}

/// Loads the first of `paths` that is a VISA library.
fn load_first(paths: Vec<PathBuf>) -> Result<Library, String> {
    let mut attempts = Vec::new();
    for path in paths {
        match Library::load(&path) {
            Ok(lib) => {
                tracing::debug!(path = %path.display(), missing = ?lib.missing(), "vendor: loaded VISA");
                return Ok(lib);
            }
            Err(e) => attempts.push(format!("  {}: {e}", path.display())),
        }
    }
    Err(format!(
        "no VISA library could be loaded. Install NI-VISA, Keysight IO Libraries \
         or R&S VISA, or set {LIBRARY_ENV} to the library's path. Tried:\n{}",
        attempts.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_dedup() {
        let explicit = PathBuf::from("/opt/a/libvisa.so");
        let list = candidates(Some(&explicit), Some(OsString::from("/opt/b/libvisa.so")));
        assert_eq!(list[0], explicit);
        assert_eq!(list[1], PathBuf::from("/opt/b/libvisa.so"));
        assert!(list.len() > 2, "platform defaults follow");

        let same = candidates(Some(&explicit), Some(explicit.clone().into_os_string()));
        assert_eq!(same.iter().filter(|p| **p == explicit).count(), 1);

        let none = candidates(None, Some(OsString::new()));
        assert_eq!(none, candidates(None, None), "empty env var is ignored");
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls dlopen")]
    fn failure_message_is_helpful() {
        // Only the bad path, not the platform defaults: on a host with VISA
        // installed, those would load.
        let err = load_first(vec![PathBuf::from("/nonexistent/visa-library")]).unwrap_err();
        assert!(err.contains("/nonexistent/visa-library"), "{err}");
        assert!(err.contains(LIBRARY_ENV), "{err}");
        assert!(err.contains("NI-VISA"), "{err}");
    }
}
