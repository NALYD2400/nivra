#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod cursor_usage;
mod browser;
use serde_json::{json, Value};
use std::{io::{BufRead, BufReader, Write}, process::{Command, Stdio}, sync::mpsc, time::Duration};
use tauri::Manager;
#[cfg(windows)] use std::os::windows::process::CommandExt;

use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}};
static LOGIN_TASKS: OnceLock<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
struct LoginGuard(String);
impl Drop for LoginGuard { fn drop(&mut self) { if let Ok(mut tasks) = LOGIN_TASKS.get_or_init(Default::default).lock() { tasks.remove(&self.0); } } }
fn register_login(profile: &str) -> Result<(Arc<AtomicBool>, LoginGuard), String> {
    let flag = Arc::new(AtomicBool::new(false));
    LOGIN_TASKS.get_or_init(Default::default).lock().map_err(|_| "Connexion indisponible")?.insert(profile.to_owned(), flag.clone());
    Ok((flag, LoginGuard(profile.to_owned())))
}
#[tauri::command]
fn cancel_login(profile: String) {
    if let Ok(tasks) = LOGIN_TASKS.get_or_init(Default::default).lock() { if let Some(flag) = tasks.get(&profile) { flag.store(true, Ordering::Relaxed); } }
}

// Prefer the Codex desktop app server when its resources directory is on PATH.
// It supports reset-credit details while older standalone CLIs may omit them.
fn codex_usage_executable() -> std::path::PathBuf {
    static EXECUTABLE: OnceLock<std::path::PathBuf> = OnceLock::new();
    EXECUTABLE.get_or_init(|| {
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                let text = dir.to_string_lossy().to_ascii_lowercase();
                if text.contains("windowsapps") && text.contains("openai.codex_") && text.ends_with("app\\resources") {
                    let candidate = dir.join("codex.exe");
                    if candidate.is_file() { return candidate; }
                }
            }
        }
        // A normal Windows launch need not inherit the desktop app's PATH.
        #[cfg(windows)]
        {
            let output = Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", "(Get-AppxPackage -Name OpenAI.Codex -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty InstallLocation)"])
                .creation_flags(0x08000000).output();
            if let Ok(output) = output {
                if output.status.success() {
                    let root = String::from_utf8_lossy(&output.stdout);
                    let candidate = std::path::Path::new(root.trim()).join("app").join("resources").join("codex.exe");
                    if candidate.is_file() { return candidate; }
                }
            }
        }
        "codex".into()
    }).clone()
}

#[tauri::command]
async fn codex_usage(app: tauri::AppHandle, profile: Option<String>) -> Result<Value, String> {
    let home = profile_home(&app, profile.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut command = Command::new(codex_usage_executable());
        command.arg("app-server");
        if let Some(home) = home { command.env("CODEX_HOME", home).args(["-c", "cli_auth_credentials_store=\"auto\""]); }
        command.args(["--listen", "stdio://"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)] command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|_| "Codex est introuvable. Installez Codex et connectez-vous, puis réessayez.".to_string())?;
        let mut input = child.stdin.take().ok_or("Impossible de contacter Codex")?;
        let output = child.stdout.take().ok_or("Impossible de lire la réponse de Codex")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || { for line in BufReader::new(output).lines().map_while(Result::ok) { if let Ok(v) = serde_json::from_str::<Value>(&line) { let _ = tx.send(v); } } });
        let send = |w: &mut std::process::ChildStdin, v: Value| -> Result<(), String> { writeln!(w, "{}", v).map_err(|e| e.to_string())?; w.flush().map_err(|e| e.to_string()) };
        let result = (|| -> Result<Value,String> {
            send(&mut input, json!({"method":"initialize","id":1,"params":{"clientInfo":{"name":"quota_hub","title":"Quota","version":"0.1.0"}}}))?;
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            loop { let v=rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).map_err(|_| "Codex ne répond pas. Vérifiez sa connexion puis réessayez.".to_string())?; if v["id"]==1 { if !v["error"].is_null() {return Err("Initialisation Codex impossible.".into())} break } }
            send(&mut input, json!({"method":"initialized","params":{}}))?;
            send(&mut input, json!({"method":"account/read","id":2,"params":{"refreshToken":false}}))?;
            send(&mut input, json!({"method":"account/rateLimits/read","id":3}))?;
            let (mut account,mut limits)=(None,None);
            while account.is_none() || limits.is_none() { let v=rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).map_err(|_| "Lecture des quotas Codex expirée. Réessayez.".to_string())?; if v["id"]==2 { account=Some(v); } else if v["id"]==3 { limits=Some(v); } }
            let a=account.unwrap(); let l=limits.unwrap();
            if !l["error"].is_null() { return Err("Codex n'a pas pu fournir les limites de ce compte.".into()); }
            Ok(json!({"account":a["result"]["account"],"limits":l["result"]}))
        })();
        let _=child.kill(); let _=child.wait(); result
    }).await.map_err(|_| "Lecture Codex interrompue".to_string())?
}

fn profile_home(app: &tauri::AppHandle, profile: Option<&str>) -> Result<Option<std::path::PathBuf>, String> {
    match profile {
        None => Ok(None),
        Some(id) if id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') => {
            let dir = app.path().app_data_dir().map_err(|_| "Dossier de l’application inaccessible".to_string())?.join("codex-profiles").join(id);
            Ok(Some(dir))
        }
        _ => Err("Identifiant de compte invalide".into()),
    }
}

fn claude_command(app: &tauri::AppHandle, profile: &str) -> Result<Command, String> {
    if profile.len() != 36 || !profile.chars().all(|c| c.is_ascii_hexdigit() || c == '-') { return Err("Identifiant de compte invalide".into()); }
    let home = app.path().app_data_dir().map_err(|_| "Dossier de l’application inaccessible".to_string())?.join("claude-profiles").join(profile);
    std::fs::create_dir_all(&home).map_err(|_| "Impossible de créer le profil Claude".to_string())?;
    #[cfg(windows)]
    let executable = std::env::var_os("APPDATA").map(std::path::PathBuf::from).map(|p| p.join("npm/node_modules/@anthropic-ai/claude-code/bin/claude.exe")).filter(|p| p.is_file()).unwrap_or_else(|| "claude.exe".into());
    #[cfg(not(windows))]
    let executable = std::path::PathBuf::from("claude");
    let mut cmd = Command::new(executable);
    cmd.env("CLAUDE_CONFIG_DIR", home)
        .env_remove("CLAUDE_CODE_USE_VERTEX").env_remove("CLAUDE_CODE_USE_BEDROCK").env_remove("CLAUDE_CODE_USE_FOUNDRY")
        .env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN").env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)] cmd.creation_flags(0x08000000);
    Ok(cmd)
}

#[tauri::command]
async fn claude_account(app: tauri::AppHandle, profile: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let output = claude_command(&app, &profile)?.args(["auth", "status", "--json"]).output().map_err(|_| "Claude Code est introuvable. Installe ou mets à jour Claude Code.".to_string())?;
        let status: Value = serde_json::from_slice(&output.stdout).map_err(|_| "Impossible de lire l’état du compte Claude.".to_string())?;
        if status["loggedIn"] != true || status["authMethod"] == "third_party" { return Err("Ce compte Claude n’est pas connecté. Reconnecte-le.".into()); }
        Ok(json!({"email": status["email"], "subscriptionType": status["subscriptionType"]}))
    }).await.map_err(|_| "Lecture Claude interrompue".to_string())?
}

#[tauri::command]
async fn login_claude(app: tauri::AppHandle, profile: String) -> Result<(), String> {
    let (cancelled, guard) = register_login(&profile)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let mut child = claude_command(&app, &profile)?.args(["auth", "login", "--claudeai"]).stdout(Stdio::null()).spawn().map_err(|_| "Impossible de lancer Claude Code. Vérifie son installation.".to_string())?;
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        loop { if cancelled.load(Ordering::Relaxed) { let _=child.kill(); let _=child.wait(); return Err("Connexion annulée.".into()); } match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err("Connexion Claude non terminée. Vérifie ton navigateur et réessaie.".into()),
            Err(_) => return Err("Impossible de suivre la connexion Claude.".into()),
            Ok(None) if std::time::Instant::now() >= deadline => { let _=child.kill(); let _=child.wait(); return Err("La connexion Claude a expiré.".into()); },
            Ok(None) => std::thread::sleep(Duration::from_millis(300)),
        }}
    }).await.map_err(|_| "Connexion Claude interrompue".to_string())?
}

#[tauri::command]
async fn disconnect_claude(app: tauri::AppHandle, profile: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let status = claude_command(&app, &profile)?.args(["auth", "logout"]).stdout(Stdio::null()).status().map_err(|_| "Impossible de déconnecter Claude.".to_string())?;
        if status.success() { Ok(()) } else { Err("La déconnexion Claude a échoué.".into()) }
    }).await.map_err(|_| "Déconnexion Claude interrompue".to_string())?
}

#[tauri::command]
async fn login_codex(app: tauri::AppHandle, profile: String) -> Result<(), String> {
    let home = profile_home(&app, Some(&profile))?.ok_or("Profil invalide")?;
    let (cancelled, guard) = register_login(&profile)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        std::fs::create_dir_all(&home).map_err(|_| "Impossible de créer le profil local".to_string())?;
        let mut cmd = Command::new("codex");
        cmd.args(["login", "-c", "cli_auth_credentials_store=\"auto\""]).env("CODEX_HOME", home).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)] cmd.creation_flags(0x08000000);
        let mut child = cmd.spawn().map_err(|_| "Impossible de lancer la connexion Codex".to_string())?;
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        loop {
            if cancelled.load(Ordering::Relaxed) { let _=child.kill(); let _=child.wait(); return Err("Connexion annulée.".into()); }
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(_)) => return Err("Connexion non terminée. Réessayez et vérifiez le navigateur et le port local Codex.".into()),
                Err(_) => return Err("Impossible de suivre la connexion Codex".into()),
                Ok(None) if std::time::Instant::now() >= deadline => { let _=child.kill(); let _=child.wait(); return Err("La connexion a expiré. Réessayez.".into()); },
                Ok(None) => std::thread::sleep(Duration::from_millis(300)),
            }
        }
    }).await.map_err(|_| "Connexion interrompue".to_string())?
}

#[tauri::command]
async fn disconnect_codex(app: tauri::AppHandle, profile: String) -> Result<(), String> {
    let home = profile_home(&app, Some(&profile))?.ok_or("Profil invalide")?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = Command::new("codex");
        cmd.args(["logout", "-c", "cli_auth_credentials_store=\"auto\""]).env("CODEX_HOME", home).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)] cmd.creation_flags(0x08000000);
        let status = cmd.status().map_err(|_| "Impossible de déconnecter ce compte".to_string())?;
        if status.success() { Ok(()) } else { Err("La déconnexion a échoué. Réessayez.".into()) }
    }).await.map_err(|_| "Déconnexion interrompue".to_string())?
}

#[tauri::command]
fn open_usage(service: &str) -> Result<(), String> {
    let url=match service { "Codex"=>"https://chatgpt.com/settings/usage", "Claude Code"=>"https://claude.ai/settings/usage", "Cursor"=>"https://cursor.com/dashboard", "xAI / Grok"=>"https://grok.com/dashboard", _=>return Err("Service inconnu".into()) };
    browser::open(url)
}
fn main(){ tauri::Builder::default().plugin(tauri_plugin_updater::Builder::new().build()).plugin(tauri_plugin_process::init()).invoke_handler(tauri::generate_handler![cursor_usage::cursor_usage,cursor_usage::login_cursor,codex_usage,login_codex,disconnect_codex,claude_account,login_claude,disconnect_claude,cancel_login,open_usage]).run(tauri::generate_context!()).expect("Erreur au lancement de Quota"); }
