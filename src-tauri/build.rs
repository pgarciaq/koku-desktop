use std::fs;
use std::process::Command;
use std::time::SystemTime;

fn main() {
    let build_date = chrono_free_date();
    println!("cargo:rustc-env=BUILD_DATE={build_date}");

    let hash = git_output(&["rev-parse", "HEAD"])
        .map(|h| h.chars().take(10).collect::<String>())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_HASH={hash}");

    let dirty = git_output(&["status", "--porcelain"])
        .map(|s| if s.is_empty() { "" } else { "-dirty" })
        .unwrap_or("");
    let build_id = format!("{build_date}.{hash}{dirty}");
    println!("cargo:rustc-env=BUILD_ID={build_id}");

    let (ui_date, ui_hash, ui_ref) = read_ui_build_info();
    println!("cargo:rustc-env=UI_DATE={ui_date}");
    println!("cargo:rustc-env=UI_HASH={ui_hash}");
    println!("cargo:rustc-env=UI_REF={ui_ref}");

    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");
    println!("cargo:rerun-if-changed=../ui/.build-info");
    println!("cargo:rerun-if-changed=../VERSION");

    tauri_build::build()
}

fn read_ui_build_info() -> (String, String, String) {
    let path = std::path::Path::new("..").join("ui").join(".build-info");
    if let Ok(contents) = fs::read_to_string(&path) {
        let parts: Vec<&str> = contents.trim().splitn(3, ' ').collect();
        let date = parts.first().unwrap_or(&"unknown").to_string();
        let hash = parts.get(1).unwrap_or(&"unknown").to_string();
        let git_ref = parts.get(2).unwrap_or(&"unknown").to_string();
        return (date, hash, git_ref);
    }
    ("unknown".into(), "unknown".into(), "unknown".into())
}

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn chrono_free_date() -> String {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = dur.as_secs() as i64;
    let days = secs / 86400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}")
}

fn civil_from_days(mut days: i64) -> (i64, u32, u32) {
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
