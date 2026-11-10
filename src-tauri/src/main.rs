#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod browser;
mod kimi_login;
mod xai_login;
mod qwen_login;
#[allow(dead_code)] mod kiro_login;
#[allow(dead_code)] mod kiro_eventstream;
#[allow(dead_code)] mod kiro_request;
#[allow(dead_code)] mod kiro_response;
#[allow(dead_code)] mod kiro_transport;
mod kiro_usage;
mod kiro_thinking;
mod nous_login;
mod grok_stream;
mod copilot_login;
mod chatgpt_adapter;
#[allow(dead_code)] mod openai_identity;
#[allow(dead_code)] mod openai_oauth;
mod gateway;
mod antigravity_tools;
mod responses_adapter;
mod messages_adapter;
mod anthropic_upstream;
mod google_request;
mod antigravity_project;
mod antigravity_tokens;
mod antigravity_login;
mod antigravity_cli;
mod openrouter_login;
mod vault;
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

#[cfg(windows)]
fn antigravity_profile(app: &tauri::AppHandle, profile: &str) -> Result<std::path::PathBuf, String> {
    if profile.len() != 36 || !profile.chars().all(|c| c.is_ascii_hexdigit() || c == '-') { return Err("Identifiant de compte invalide".into()); }
    Ok(app.path().app_data_dir().map_err(|_| "Dossier de l’application inaccessible".to_string())?.join("antigravity-profiles").join(profile))
}

#[cfg(windows)]
fn open_antigravity(home: &std::path::Path) -> Result<(), String> {
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).map(|p| p.join("Programs/Antigravity/Antigravity.exe"));
    let program = std::env::var_os("PROGRAMFILES").map(std::path::PathBuf::from).map(|p| p.join("Antigravity/Antigravity.exe"));
    let executable = [local, program].into_iter().flatten().find(|p| p.is_file()).ok_or("Antigravity est introuvable. Installe l’application Antigravity.")?;
    Command::new(executable).arg("--user-data-dir").arg(home).arg("--new-window").spawn().map_err(|_| "Impossible d’ouvrir le profil Antigravity".to_string())?;
    Ok(())
}

#[tauri::command]
fn open_antigravity_profile(app: tauri::AppHandle, profile: String) -> Result<(), String> {
    #[cfg(windows)] { let home = antigravity_profile(&app, &profile)?; if !home.is_dir() { return Err("Profil Antigravity introuvable.".into()); } open_antigravity(&home) }
    #[cfg(not(windows))] { let _ = (app, profile); Err("Profil Antigravity disponible uniquement sur Windows.".into()) }
}

#[tauri::command]
async fn login_antigravity(app: tauri::AppHandle, profile: String) -> Result<String, String> {
    let (cancelled, guard) = register_login(&profile)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        #[cfg(windows)] {
            let home = antigravity_profile(&app, &profile)?;
            std::fs::create_dir_all(&home).map_err(|_| "Impossible de créer le profil Antigravity".to_string())?;
            open_antigravity(&home)?;
            let deadline = std::time::Instant::now() + Duration::from_secs(300);
            while std::time::Instant::now() < deadline {
                if cancelled.load(Ordering::Relaxed) { return Err("Connexion annulée.".into()); }
                if let Ok(email) = local_account::email("Antigravity", Some(&home)) { return Ok(email); }
                std::thread::sleep(Duration::from_secs(2));
            }
            Err("Aucun compte détecté dans le profil Antigravity. Connecte-toi dans la fenêtre ouverte puis réessaie.".into())
        }
        #[cfg(not(windows))] { let _ = (app, profile, cancelled); Err("Connexion Antigravity disponible uniquement sur Windows.".into()) }
    }).await.map_err(|_| "Connexion Antigravity interrompue".to_string())?
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

#[derive(serde::Serialize,Debug,PartialEq)]
struct CodexModel { id:String, name:String, is_default:bool }
fn parse_codex_models(result:&Value)->Result<(Vec<CodexModel>,Option<String>),String>{
    let rows=result.get("data").and_then(|v|v.as_array()).filter(|v|v.len()<=200).ok_or("Catalogue Codex illisible.")?;
    let mut models=Vec::new();for item in rows{
        let Some(id)=item.get("model").and_then(|v|v.as_str()).filter(|s|!s.is_empty()&&s.len()<=200&&!s.chars().any(char::is_control)) else{continue};
        let name=item.get("displayName").and_then(|v|v.as_str()).filter(|s|!s.is_empty()&&s.len()<=200&&!s.chars().any(char::is_control)).unwrap_or(id);
        if item.get("hidden").and_then(|v|v.as_bool())==Some(true){continue}
        models.push(CodexModel{id:id.into(),name:name.into(),is_default:item.get("isDefault").and_then(|v|v.as_bool()).unwrap_or(false)});
    }
    let cursor=match result.get("nextCursor"){None|Some(Value::Null)=>None,Some(Value::String(s)) if !s.is_empty()&&s.len()<=2048&&!s.chars().any(char::is_control)=>Some(s.clone()),_=>return Err("Pagination Codex invalide.".into())};
    Ok((models,cursor))
}
#[tauri::command]
async fn codex_profile_models(app:tauri::AppHandle,profile:String)->Result<Vec<CodexModel>,String>{
    if uuid::Uuid::parse_str(&profile).is_err()||profile.len()!=36{return Err("Profil Codex invalide.".into())}
    let home=profile_home(&app,Some(&profile))?.ok_or("Profil Codex invalide.")?;if !home.is_dir(){return Err("Profil Codex introuvable.".into())}
    tauri::async_runtime::spawn_blocking(move||{
        let mut command=Command::new("codex");command.arg("app-server").env("CODEX_HOME",home).args(["-c","cli_auth_credentials_store=\"auto\"","--listen","stdio://"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)] command.creation_flags(0x08000000);
        let mut child=command.spawn().map_err(|_|"Impossible de lancer Codex.")?;
        let result=(||->Result<Vec<CodexModel>,String>{
            let mut input=child.stdin.take().ok_or("Entrée Codex indisponible.")?;let output=child.stdout.take().ok_or("Sortie Codex indisponible.")?;let(tx,rx)=mpsc::sync_channel::<Value>(32);
            std::thread::spawn(move||{for line in BufReader::new(output).lines().map_while(Result::ok){if line.len()>2_000_000{break}if let Ok(value)=serde_json::from_str::<Value>(&line){if tx.send(value).is_err(){break}}}});
            let send=|input:&mut std::process::ChildStdin,value:Value|->Result<(),String>{writeln!(input,"{}",value).map_err(|_|"Communication Codex interrompue.")?;input.flush().map_err(|_|"Communication Codex interrompue.".into())};
            let deadline=std::time::Instant::now()+Duration::from_secs(20);
            let receive=|id:u64|->Result<Value,String>{loop{let value=rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).map_err(|_|"Lecture des modèles Codex expirée.")?;if value.get("id").and_then(|v|v.as_u64())==Some(id){if !value["error"].is_null(){return Err("Codex n’a pas pu lire les modèles.".into())}return Ok(value["result"].clone())}}};
            send(&mut input,json!({"method":"initialize","id":1,"params":{"clientInfo":{"name":"nivra","title":"Nivra","version":"0.1.0"}}}))?;receive(1)?;send(&mut input,json!({"method":"initialized","params":{}}))?;
            let mut cursor=None;let mut seen=std::collections::HashSet::new();let mut models=Vec::new();
            for page in 0..10u64{send(&mut input,json!({"method":"model/list","id":page+2,"params":{"cursor":cursor,"limit":100,"includeHidden":false}}))?;let (entries,next)=parse_codex_models(&receive(page+2)?)?;for item in entries{if seen.insert(item.id.clone()){models.push(item)}}if models.len()>500{return Err("Catalogue Codex trop volumineux.".into())}match next{Some(next) if cursor.as_ref()!=Some(&next)=>cursor=Some(next),Some(_)=>return Err("Pagination Codex répétée.".into()),None=>return Ok(models)}}Err("Catalogue Codex trop volumineux.".into())
        })();let _=child.kill();let _=child.wait();result
    }).await.map_err(|_|"Lecture des modèles Codex interrompue.".to_string())?
}

#[cfg(test)] mod codex_models_tests{
    use super::*;
    #[test] fn accepts_visible_catalog_and_bounded_cursor(){let (rows,cursor)=parse_codex_models(&json!({"data":[{"model":"m1","displayName":"Modèle 1","isDefault":true},{"model":"hidden","hidden":true},{"model":"bad\nname"}],"nextCursor":"next"})).unwrap();assert_eq!(rows,vec![CodexModel{id:"m1".into(),name:"Modèle 1".into(),is_default:true}]);assert_eq!(cursor.as_deref(),Some("next"));assert!(parse_codex_models(&json!({"data":[],"nextCursor":"bad\n"})).is_err());assert!(parse_codex_models(&json!({"data":null})).is_err());}
}

#[cfg(windows)]
fn codex_profile_command(executable:&std::path::Path,home:&std::path::Path,workspace:&std::path::Path)->Command {
    let mut command=Command::new(executable);
    command.args(["-c","cli_auth_credentials_store=\"auto\""]).env("CODEX_HOME",home)
        .env_remove("OPENAI_API_KEY").env_remove("OPENAI_BASE_URL").current_dir(workspace).creation_flags(0x00000010);
    command
}

#[tauri::command]
fn open_codex_profile(app:tauri::AppHandle,profile:String)->Result<(),String>{
    #[cfg(windows)] {
        let home=profile_home(&app,Some(&profile))?.ok_or("Profil invalide.")?;
        if !home.is_dir(){return Err("Profil Codex introuvable. Reconnecte ce compte.".into())}
        let workspace=app.path().document_dir().map_err(|_|"Dossier Documents inaccessible.")?;
        codex_profile_command(&gateway::codex_executable()?,&home,&workspace).spawn().map_err(|_|"Impossible d’ouvrir Codex.".to_string())?;
        Ok(())
    }
    #[cfg(not(windows))] {let _=(app,profile);Err("Ouverture de profil disponible uniquement sur Windows.".into())}
}

#[cfg(all(test,windows))]
mod profile_launch_tests {
    #[test] fn profile_launch_selects_only_the_requested_home(){
        let command=super::codex_profile_command(std::path::Path::new("C:\\codex.exe"),std::path::Path::new("C:\\profile-a"),std::path::Path::new("C:\\project"));
        let env:std::collections::HashMap<_,_>=command.get_envs().map(|(k,v)|(k.to_string_lossy().into_owned(),v.map(|x|x.to_string_lossy().into_owned()))).collect();
        assert_eq!(env["CODEX_HOME"].as_deref(),Some("C:\\profile-a"));assert_eq!(env["OPENAI_API_KEY"],None);assert_eq!(env["OPENAI_BASE_URL"],None);
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

#[cfg(windows)]
fn claude_profile_launch_command(executable:&std::path::Path,home:&std::path::Path,workspace:&std::path::Path)->Command{
    let mut command=Command::new(executable);
    command.env("CLAUDE_CONFIG_DIR",home).env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN").env_remove("ANTHROPIC_BASE_URL")
        .env_remove("CLAUDE_CODE_USE_VERTEX").env_remove("CLAUDE_CODE_USE_BEDROCK").env_remove("CLAUDE_CODE_USE_FOUNDRY")
        .current_dir(workspace).creation_flags(0x00000010);
    command
}

#[tauri::command]
fn open_claude_profile(app:tauri::AppHandle,profile:String)->Result<(),String>{
    #[cfg(windows)] {
        if uuid::Uuid::parse_str(&profile).is_err() || profile.len()!=36{return Err("Identifiant de compte invalide.".into())}
        let home=app.path().app_data_dir().map_err(|_|"Dossier de l’application inaccessible.")?.join("claude-profiles").join(profile);
        if !home.is_dir(){return Err("Profil Claude introuvable. Reconnecte ce compte.".into())}
        let workspace=app.path().document_dir().map_err(|_|"Dossier Documents inaccessible.")?;
        claude_profile_launch_command(&gateway::claude_executable()?,&home,&workspace).spawn().map_err(|_|"Impossible d’ouvrir Claude Code.".to_string())?;
        Ok(())
    }
    #[cfg(not(windows))] {let _=(app,profile);Err("Ouverture de profil disponible uniquement sur Windows.".into())}
}

#[cfg(all(test,windows))]
#[test]
fn claude_profile_launch_isolates_credentials(){
    let command=claude_profile_launch_command(std::path::Path::new("C:\\claude.exe"),std::path::Path::new("C:\\profile-a"),std::path::Path::new("C:\\project"));
    let env:std::collections::HashMap<_,_>=command.get_envs().map(|(k,v)|(k.to_string_lossy().into_owned(),v.map(|x|x.to_string_lossy().into_owned()))).collect();
    assert_eq!(env["CLAUDE_CONFIG_DIR"].as_deref(),Some("C:\\profile-a"));
    for key in ["ANTHROPIC_API_KEY","ANTHROPIC_AUTH_TOKEN","CLAUDE_CODE_OAUTH_TOKEN","ANTHROPIC_BASE_URL","CLAUDE_CODE_USE_VERTEX","CLAUDE_CODE_USE_BEDROCK","CLAUDE_CODE_USE_FOUNDRY"]{assert_eq!(env[key],None)}
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
    browser::open(url)
}
fn main(){ tauri::Builder::default().manage(gateway::GatewayManager::default()).plugin(tauri_plugin_updater::Builder::new().build()).plugin(tauri_plugin_process::init()).invoke_handler(tauri::generate_handler![codex_usage,codex_profile_models,login_codex,disconnect_codex,open_codex_profile,claude_account,login_claude,disconnect_claude,open_claude_profile,existing_service_account,antigravity_usage,login_cursor,open_cursor_profile,login_antigravity,open_antigravity_profile,antigravity_cli::antigravity_cli_models,antigravity_cli::antigravity_cli_launch,antigravity_cli::antigravity_cli_login,antigravity_login::antigravity_gateway_login,antigravity_login::antigravity_gateway_cancel,antigravity_login::antigravity_gateway_check,antigravity_login::antigravity_try_model,openrouter_login::openrouter_login,openai_oauth::chatgpt_plan_login,openai_oauth::chatgpt_plan_forget,kimi_login::kimi_login_start,kimi_login::kimi_login_finish,kimi_login::kimi_login_cancel,kimi_login::kimi_usage,nous_login::nous_login_start,nous_login::nous_login_finish,nous_login::nous_login_cancel,qwen_login::qwen_login_start,qwen_login::qwen_login_finish,qwen_login::qwen_login_cancel,kiro_login::kiro_builder_start,kiro_login::kiro_builder_finish,kiro_login::kiro_builder_cancel,kiro_login::kiro_builder_models,kiro_usage::kiro_builder_usage,xai_login::xai_login,xai_login::xai_usage,copilot_login::copilot_login_start,copilot_login::copilot_login_finish,copilot_login::copilot_login_cancel,copilot_login::copilot_usage,cancel_login,open_usage,gateway::gateway_start,gateway::gateway_start_antigravity,gateway::gateway_start_nous,gateway::gateway_add_nous,gateway::gateway_start_qwen,gateway::gateway_add_qwen,gateway::gateway_start_kiro,gateway::gateway_add_kiro,gateway::gateway_start_kimi,gateway::gateway_start_xai,gateway::gateway_start_copilot,gateway::gateway_start_chatgpt,gateway::gateway_stop,gateway::gateway_status,gateway::gateway_models,gateway::gateway_providers,gateway::gateway_openrouter_usage,gateway::gateway_deepseek_balance,gateway::gateway_glm_quota,gateway::gateway_opencode_go_usage,gateway::gateway_cline_usage,gateway::gateway_launch_codex,gateway::gateway_launch_claude,gateway::gateway_add_provider,gateway::gateway_add_antigravity,gateway::gateway_add_kimi,gateway::gateway_add_xai,gateway::gateway_add_copilot,gateway::gateway_add_chatgpt,gateway::gateway_remove_provider,vault::gateway_remember_key,vault::gateway_forget_key]).run(tauri::generate_context!()).expect("Erreur au lancement de Quota"); }
