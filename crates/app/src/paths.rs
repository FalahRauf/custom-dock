use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NACRE_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("CustomDock");
    migrate_legacy(&base.join("ABRA"), &dir);
    migrate_legacy(&base.join("Nacre"), &dir);
    dir
}

fn migrate_legacy(old: &std::path::Path, new: &std::path::Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if new.exists() || !old.exists() {
            return;
        }
        let _ = std::fs::create_dir_all(new);
        copy_tree(old, new);
    });
}

pub fn marker() -> PathBuf {
    config_dir().join("taskbar_hidden.flag")
}

pub fn log() -> PathBuf {
    config_dir().join("abra.log")
}

pub fn guard_exe() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("custom-dock-guard.exe")))
        .unwrap_or_else(|| PathBuf::from("custom-dock-guard.exe"))
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    let _ = std::fs::create_dir_all(to);
    let Ok(rd) = std::fs::read_dir(from) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().ends_with(".flag") {
            continue;
        }
        if p.is_dir() {
            copy_tree(&p, &to.join(&name));
        } else {
            let _ = std::fs::copy(&p, to.join(&name));
        }
    }
}
