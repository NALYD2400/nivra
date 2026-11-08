#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
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

#[cfg(windows)]
mod local_account {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    #[link(name = "winsqlite3")]
    extern "C" {
        fn sqlite3_open_v2(path: *const c_char, db: *mut *mut c_void, flags: c_int, vfs: *const c_char) -> c_int;
        fn sqlite3_prepare_v2(db: *mut c_void, sql: *const c_char, n: c_int, stmt: *mut *mut c_void, tail: *mut *const c_char) -> c_int;
        fn sqlite3_step(stmt: *mut c_void) -> c_int;
        fn sqlite3_column_text(stmt: *mut c_void, col: c_int) -> *const u8;
        fn sqlite3_finalize(stmt: *mut c_void) -> c_int;
        fn sqlite3_close(db: *mut c_void) -> c_int;
    }
    pub fn email(service: &str, profile: Option<&std::path::Path>) -> Result<String, String> {
        let (folder, query) = match service {
            "Cursor" => ("Cursor", "SELECT value FROM ItemTable WHERE key = 'cursorAuth/cachedEmail' LIMIT 1"),
            "Antigravity" => ("Antigravity", "SELECT value FROM ItemTable WHERE key = 'antigravityAuthStatus' LIMIT 1"),
            _ => return Err("Service inconnu".into()),
        };
        let path = std::env::var_os("APPDATA").ok_or("Dossier Windows inaccessible")?;
        let path = profile.map(std::path::Path::to_path_buf).unwrap_or_else(|| std::path::PathBuf::from(path).join(folder)).join("User/globalStorage/state.vscdb");
        if !path.is_file() { return Err(format!("Ouvre {folder} et connecte ton compte, puis réessaie.")); }
        let path = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "Chemin invalide")?;
        let query = CString::new(query).map_err(|_| "Requête invalide")?;
        unsafe {
            let mut db = std::ptr::null_mut();
            if sqlite3_open_v2(path.as_ptr(), &mut db, 1, std::ptr::null()) != 0 { if !db.is_null() { sqlite3_close(db); } return Err("Impossible de lire le compte local.".into()); }
            let mut stmt = std::ptr::null_mut();
            let result = if sqlite3_prepare_v2(db, query.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) == 0 && sqlite3_step(stmt) == 100 {
                let value = sqlite3_column_text(stmt, 0);
                if value.is_null() { None } else {
                    let value = CStr::from_ptr(value.cast()).to_string_lossy();
                    if service == "Cursor" { Some(value.into_owned()) } else { serde_json::from_str::<serde_json::Value>(&value).ok().and_then(|v| v["email"].as_str().map(str::to_owned)) }
                }
            } else { None };
            if !stmt.is_null() { sqlite3_finalize(stmt); }
            sqlite3_close(db);
            result.filter(|email| email.contains('@') && email.len() <= 320).ok_or_else(|| format!("Aucun compte trouvé. Connecte-toi dans {folder}, puis réessaie."))
        }
    }
}

#[tauri::command]
async fn existing_service_account(service: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)] { local_account::email(&service, None) }
        #[cfg(not(windows))] { let _ = service; Err("Import disponible uniquement sur Windows.".into()) }
    }).await.map_err(|_| "Lecture du compte interrompue".to_string())?
}

#[tauri::command]
async fn antigravity_usage() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)] {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-NonInteractive", "-Command", include_str!("antigravity_usage.ps1")]).stdin(Stdio::null()).stderr(Stdio::null());
            command.creation_flags(0x08000000);
            let output = command.output().map_err(|_| "Lecture Antigravity indisponible.".to_string())?;
            if !output.status.success() { return Err("Quotas indisponibles. Vérifie qu’Antigravity est ouvert et connecté.".into()); }
            serde_json::from_slice(&output.stdout).map_err(|_| "Réponse Antigravity illisible.".to_string())
        }
        #[cfg(not(windows))] { Err("Lecture Antigravity disponible uniquement sur Windows.".into()) }
    }).await.map_err(|_| "Lecture Antigravity interrompue".to_string())?
}

#[cfg(windows)]
fn cursor_profile(app: &tauri::AppHandle, profile: &str) -> Result<std::path::PathBuf, String> {
    if profile.len() != 36 || !profile.chars().all(|c| c.is_ascii_hexdigit() || c == '-') { return Err("Identifiant de compte invalide".into()); }
    Ok(app.path().app_data_dir().map_err(|_| "Dossier de l’application inaccessible".to_string())?.join("cursor-profiles").join(profile))
}

#[cfg(windows)]
fn open_cursor(home: &std::path::Path) -> Result<(), String> {
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).map(|p| p.join("Programs/cursor/Cursor.exe"));
    let program = std::env::var_os("PROGRAMFILES").map(std::path::PathBuf::from).map(|p| p.join("cursor/Cursor.exe"));
    let executable = [local, program].into_iter().flatten().find(|p| p.is_file()).ok_or("Cursor est introuvable. Installe l’application Cursor.")?;
    Command::new(executable).arg("--user-data-dir").arg(home).arg("--new-window").spawn().map_err(|_| "Impossible d’ouvrir le profil Cursor".to_string())?;
    Ok(())
}

#[tauri::command]
fn open_cursor_profile(app: tauri::AppHandle, profile: String) -> Result<(), String> {
    #[cfg(windows)] { let home = cursor_profile(&app, &profile)?; if !home.is_dir() { return Err("Profil Cursor introuvable.".into()); } open_cursor(&home) }
    #[cfg(not(windows))] { let _ = (app, profile); Err("Profil Cursor disponible uniquement sur Windows.".into()) }
}

#[tauri::command]
async fn login_cursor(app: tauri::AppHandle, profile: String) -> Result<String, String> {
    let (cancelled, guard) = register_login(&profile)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        #[cfg(windows)] {
            let home = cursor_profile(&app, &profile)?;
            std::fs::create_dir_all(&home).map_err(|_| "Impossible de créer le profil Cursor".to_string())?;
            open_cursor(&home)?;
            let deadline = std::time::Instant::now() + Duration::from_secs(300);
            while std::time::Instant::now() < deadline {
                if cancelled.load(Ordering::Relaxed) { return Err("Connexion annulée.".into()); }
                if let Ok(email) = local_account::email("Cursor", Some(&home)) { return Ok(email); }
                std::thread::sleep(Duration::from_secs(2));
            }
            Err("Aucun compte détecté dans le profil Cursor. Réessaie et connecte-toi dans la fenêtre ouverte.".into())
        }
        #[cfg(not(windows))] { let _ = (app, profile); Err("Connexion Cursor disponible uniquement sur Windows.".into()) }
    }).await.map_err(|_| "Connexion Cursor interrompue".to_string())?
}

#[tauri::command]
async fn codex_usage(app: tauri::AppHandle, profile: Option<String>) -> Result<Value, String> {
    let home = profile_home(&app, profile.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut command = Command::new("codex");
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
    let url=match service { "Codex"=>"https://chatgpt.com/settings/usage", "Claude Code"=>"https://claude.ai/settings/usage", "Cursor"=>"https://cursor.com/settings", "Antigravity"=>"https://antigravity.google/", _=>return Err("Service inconnu".into()) };
    let mut cmd=Command::new("explorer.exe"); cmd.arg(url);
    #[cfg(windows)] cmd.creation_flags(0x08000000);
    cmd.spawn().map_err(|_| "Impossible d'ouvrir le navigateur".to_string())?; Ok(())
}
fn main(){ tauri::Builder::default().plugin(tauri_plugin_updater::Builder::new().build()).plugin(tauri_plugin_process::init()).invoke_handler(tauri::generate_handler![codex_usage,login_codex,disconnect_codex,claude_account,login_claude,disconnect_claude,existing_service_account,antigravity_usage,login_cursor,open_cursor_profile,cancel_login,open_usage]).run(tauri::generate_context!()).expect("Erreur au lancement de Quota"); }
