fn main() {
    let local = std::path::Path::new("../.private/antigravity-client.env");
    println!("cargo:rerun-if-changed={}", local.display());
    let contents = std::fs::read_to_string(local).unwrap_or_default();
    for name in ["NIVRA_ANTIGRAVITY_CLIENT_ID", "NIVRA_ANTIGRAVITY_CLIENT_SECRET"] {
        println!("cargo:rerun-if-env-changed={name}");
        let value = std::env::var(name).ok().or_else(|| contents.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key == name).then(|| value.to_owned())
        }));
        if let Some(value) = value.filter(|value| !value.is_empty() && !value.chars().any(char::is_control)) {
            println!("cargo:rustc-env={name}={value}");
        }
    }
    tauri_build::build();
}
