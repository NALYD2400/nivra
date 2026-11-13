//! Read-only usage from a dedicated official sign-in window.
//! No editor database, external browser profile, model request or renderer credential.
use serde_json::{json, Value};
use zeroize::Zeroizing;
use std::{io::Read, sync::{Mutex, OnceLock, atomic::Ordering}, time::{Duration, Instant}};
use tauri::{Manager, WebviewWindow, WebviewWindowBuilder, WebviewUrl};

const ORIGIN: &str = "https://cursor.com";
const DASHBOARD: &str = "https://cursor.com/dashboard";
const WINDOW: &str = "cursor-account";
static CONNECTING: OnceLock<Mutex<()>> = OnceLock::new();

fn account_window(app: &tauri::AppHandle, visible: bool) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(WINDOW) {
        if visible {
            window.navigate(DASHBOARD.parse().unwrap()).map_err(|_| "Impossible d’ouvrir la page officielle.")?;
            window.show().map_err(|_| "Impossible d’afficher la connexion.")?;
            let _ = window.set_focus();
        }
        return Ok(window);
    }
    let directory = app.path().app_data_dir().map_err(|_| "Profil de connexion inaccessible.")?.join("cursor-web-session");
    // Reading the persistent cookie store does not require loading the full dashboard.
    // The official page is still opened when the user starts an interactive login.
    let initial_url = if visible { DASHBOARD } else { "about:blank" };
    WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(initial_url.parse().unwrap()))
        .title("Connexion Cursor / xAI — Nivra")
        .inner_size(920.0, 760.0).min_inner_size(600.0, 500.0)
        .visible(visible).data_directory(directory)
        // This remote window has no IPC capability (only `main` is authorized).
        .on_navigation(|url| url.scheme() == "https" || url.as_str() == "about:blank")
        .build().map_err(|_| "Impossible de créer la fenêtre de connexion officielle.".to_string())
}

fn cookie_header<'a>(cookies: impl IntoIterator<Item = (&'a str, &'a str, Option<&'a str>)>) -> Option<Zeroizing<String>> {
    let names = ["WorkosCursorSessionToken", "__Secure-next-auth.session-token", "next-auth.session-token", "__Secure-authjs.session-token", "authjs.session-token"];
    let mut header = Zeroizing::new(String::new());
    for (name, value, domain) in cookies {
        if domain.is_some_and(|domain| domain.trim_start_matches('.') != "cursor.com") { continue; }
        let selected = names.iter().any(|base| name == *base || name.strip_prefix(base).and_then(|suffix| suffix.strip_prefix('.')).is_some_and(|chunk| !chunk.is_empty() && chunk.len() <= 2 && chunk.bytes().all(|b| b.is_ascii_digit())));
        if !selected || value.is_empty() || value.len() > 16384 || !value.bytes().all(|b| b.is_ascii_graphic() && b != b';') { continue; }
        if !header.is_empty() { header.push_str("; "); }
        header.push_str(name); header.push('='); header.push_str(value);
        if header.len() > 65536 { return None; }
    }
    (!header.is_empty()).then_some(header)
}

fn window_cookie(window: &WebviewWindow) -> Result<Option<Zeroizing<String>>, String> {
    // Read only session cookies applicable to cursor.com, never cookies of identity providers.
    let cookies = window.cookies_for_url(ORIGIN.parse().unwrap()).map_err(|_| "La session de connexion est inaccessible.")?;
    Ok(cookie_header(cookies.iter().map(|cookie| (cookie.name(), cookie.value(), cookie.domain()))))
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12)).build().map_err(|_| "Connexion réseau indisponible.".into())
}

fn read_json(request: reqwest::blocking::RequestBuilder) -> Result<Value, String> {
    let response = request.send().map_err(|_| "Impossible de joindre Cursor. Vérifie ta connexion Internet.")?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 | 403 => "Ta session a expiré ou a été refusée. Clique sur Reconnecter pour te connecter à nouveau.".into(),
            status => format!("Le service ne peut pas fournir le quota pour le moment (HTTP {status}). Réessaie plus tard."),
        });
    }
    let mut bytes = Vec::new();
    response.take(1_048_577).read_to_end(&mut bytes).map_err(|_| "Lecture Cursor interrompue.")?;
    if bytes.len() > 1_048_576 { return Err("Réponse Cursor trop volumineuse.".into()); }
    serde_json::from_slice(&bytes).map_err(|_| "Le service a renvoyé une réponse illisible.".into())
}

fn percent(value:Option<&Value>)->Option<f64>{value.and_then(Value::as_f64).filter(|v|v.is_finite()&&(0.0..=100.0).contains(v))}
fn text(value:Option<&Value>)->Option<String>{value.and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=100&&!s.chars().any(char::is_control)).map(str::to_owned)}
fn plan_name(summary:&Value,sand:Option<&Value>)->Option<String>{
 let unified=sand.and_then(|v|v.get("isUnified")).and_then(Value::as_bool)==Some(true);
 // cursorPlanName is the subscription tier; grokPlanLabel may merely say "Grok Bot Plan".
 ["cursorPlanName","grokPlanLabel"].into_iter()
  .find_map(|key|text(sand.and_then(|v|v.get(key))).map(|v|v.trim().to_owned())
   .filter(|v|!v.is_empty()&&!v.eq_ignore_ascii_case("Grok Bot Plan")))
  .or_else(||text(summary.get("membershipType")).map(|v|v.trim().to_owned())
   .filter(|v|!v.is_empty()&&(!unified||matches!(v.to_ascii_lowercase().as_str(),"free"|"plus"|"premium"|"super"|"ultra"))))
}
fn parse(summary:&Value,sand:Option<&Value>,email:&str,id:&str)->Value{
 let unified=sand.and_then(|v|v.get("isUnified")).and_then(Value::as_bool);
 let mut windows=Vec::new();
 if unified==Some(true){
  windows.push(json!({"kind":"shared","label":"Réserve commune Cursor · Grok · Grok Bot","used":percent(sand.and_then(|v|v.get("usagePercent"))),"reset":text(sand.and_then(|v|v.get("nextResetTimestampUtc"))),"durationMins":10080}));
 }else{
  let plan=summary.pointer("/individualUsage/plan");
  for (key,label) in [("totalPercentUsed","Total Cursor"),("autoPercentUsed","Modèles Cursor"),("apiPercentUsed","Autres modèles")]{
   let used=percent(plan.and_then(|v|v.get(key)));
   if used.is_some(){windows.push(json!({"kind":key,"label":label,"used":used,"reset":text(summary.get("billingCycleEnd")),"start":text(summary.get("billingCycleStart"))}));}
  }
  if let Some(sand)=sand{if sand.get("includedLimitZero").and_then(Value::as_bool)==Some(false)||sand.get("hasNonZeroIncludedLimit").and_then(Value::as_bool)==Some(true){windows.push(json!({"kind":"grok-bot","label":"Grok Bot · quota communiqué par Cursor","used":percent(sand.get("usagePercent")),"reset":text(sand.get("nextResetTimestampUtc")),"durationMins":10080}));}}
 }
 json!({"id":id,"email":email,"plan":plan_name(summary,sand),"unified":unified,"windows":windows})
}

fn fetch_cookie(client: &reqwest::blocking::Client, base: &str, cookie: &str) -> Result<Value, String> {
    let request = |path: &str| client.get(format!("{base}{path}")).header("cookie", cookie).header("accept", "application/json");
    let identity = read_json(request("/api/auth/me"))?;
    let id = identity.get("sub").or_else(|| identity.get("id")).and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= 320 && !v.chars().any(char::is_control))
        .ok_or("Le service n’a pas confirmé l’identité du compte.")?;
    let email = identity.get("email").and_then(Value::as_str)
        .filter(|v| v.contains('@') && v.len() <= 320 && !v.chars().any(char::is_control))
        .ok_or("Le service n’a pas confirmé l’adresse du compte.")?;
    let summary = read_json(request("/api/usage-summary"));
    let sand = read_json(client.post(format!("{base}/api/dashboard/get-sand-usage-status"))
        .header("cookie", cookie).header("origin", base).header("content-type", "application/json").body("{}"));
    if summary.is_err() && sand.is_err() { return Err(summary.unwrap_err()); }
    let mut data = parse(summary.as_ref().unwrap_or(&Value::Null), sand.as_ref().ok(), email, id);
    data["warning"] = json!(if sand.is_err() { Some("La réserve hebdomadaire n’a pas été communiquée par le service.") }
        else if summary.is_err() { Some("Les détails de l’abonnement Cursor sont momentanément indisponibles.") } else { None });
    Ok(data)
}

#[tauri::command]
pub async fn cursor_usage(app: tauri::AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Do not close the temporary reader while an interactive login is using it.
        let _lock = CONNECTING.get_or_init(Default::default).try_lock().map_err(|_| "Une connexion est déjà en cours. Réessaie dans un instant.")?;
        let window = account_window(&app, false)?;
        let result = (|| {
            // WebView2 loads the persistent cookie store asynchronously on creation.
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                if let Some(cookie) = window_cookie(&window)? { return Ok(cookie); }
                if Instant::now() >= deadline { return Err("Connecte ton compte dans Nivra pour consulter ton quota.".to_string()); }
                std::thread::sleep(Duration::from_millis(200));
            }
        })();
        // The protected profile stays on disk; only the temporary reader is closed.
        let _ = window.close();
        let cookie = result?;
        fetch_cookie(&client()?, ORIGIN, &cookie)
    }).await.map_err(|_| "Lecture du quota interrompue.".to_string())?
}

#[tauri::command]
pub async fn login_cursor(app: tauri::AppHandle, profile: String) -> Result<Value, String> {
    let (cancelled, guard) = crate::register_login(&profile)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let _lock = CONNECTING.get_or_init(Default::default).try_lock().map_err(|_| "Une connexion est déjà en cours. Réessaie dans un instant.")?;
        if cancelled.load(Ordering::Relaxed) { return Err("Connexion annulée.".into()); }
        let window = account_window(&app, true)?;
        let result = (|| {
            let deadline = Instant::now() + Duration::from_secs(240);
            let client = client()?;
            let mut last_cookie = Zeroizing::new(String::new());
            let mut last_attempt = Instant::now() - Duration::from_secs(5);
            let mut last_error = None;
            loop {
                if cancelled.load(Ordering::Relaxed) { return Err("Connexion annulée.".into()); }
                if app.get_webview_window(WINDOW).is_none() { return Err("Connexion annulée. Tu peux réessayer quand tu veux.".into()); }
                if Instant::now() >= deadline {
                    return Err(last_error.unwrap_or_else(|| "La connexion n’a pas été terminée. Réessaie et termine-la dans la fenêtre officielle.".into()));
                }
                if let Some(cookie) = window_cookie(&window)? {
                    if *cookie != *last_cookie || last_attempt.elapsed() >= Duration::from_secs(15) {
                        last_cookie = cookie;
                        last_attempt = Instant::now();
                        match fetch_cookie(&client, ORIGIN, &last_cookie) {
                            Ok(data) => {
                                if cancelled.load(Ordering::Relaxed) { return Err("Connexion annulée.".into()); }
                                return Ok(data);
                            },
                            Err(error) => last_error = Some(error),
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })();
        // Retain the protected web profile; never copy cookies into localStorage or app JSON.
        let _ = window.close();
        result
    }).await.map_err(|_| "Connexion Cursor interrompue.".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_prefers_the_current_label_and_never_guesses_from_usage(){
        let summary=json!({"membershipType":"pro"});
        assert_eq!(plan_name(&summary,Some(&json!({"isUnified":true,"grokPlanLabel":"Grok Bot Plan","cursorPlanName":"Premium"}))).as_deref(),Some("Premium"));
        assert_eq!(plan_name(&summary,Some(&json!({"isUnified":true,"grokPlanLabel":"Grok Bot Plan"}))),None);
        assert_eq!(plan_name(&json!({}),Some(&json!({"isUnified":true,"grokPlanLabel":"Grok Bot Plan"}))),None);
        assert_eq!(plan_name(&summary,Some(&json!({"cursorPlanName":"Ultra","grokPlanLabel":"SuperGrok"}))).as_deref(),Some("Ultra"));
        assert_eq!(plan_name(&summary,None).as_deref(),Some("pro"));
        assert_eq!(plan_name(&json!({}),Some(&json!({"usagePercent":0.3,"grokPlanLabel":" "}))),None);
    }
    #[test]
    fn only_cursor_session_cookies_are_sent() {
        let cookies = [
            ("WorkosCursorSessionToken", "safe-session", Some(".cursor.com")),
            ("sso", "identity-provider-secret", Some("accounts.x.ai")),
            ("WorkosCursorSessionToken", "foreign-secret", Some("evilcursor.com")),
            ("analytics", "tracking", Some("cursor.com")),
            ("next-auth.session-token.0", "chunk0", Some("cursor.com")),
            ("next-auth.session-token.1", "chunk1", Some("cursor.com")),
            ("next-auth.session-token.evil", "invalid", Some("cursor.com")),
            ("authjs.session-token", "injected;other=1", Some("cursor.com")),
            ("authjs.session-token", "injected\r\n", Some("cursor.com")),
        ];
        assert_eq!(cookie_header(cookies).unwrap().as_str(), "WorkosCursorSessionToken=safe-session; next-auth.session-token.0=chunk0; next-auth.session-token.1=chunk1");
        assert!(cookie_header([]).is_none());
    }
    #[test]
    fn verifies_account_before_requesting_usage_and_never_generates() {
        use std::io::{BufRead, BufReader, Write};
        for invalid in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let task = std::thread::spawn(move || {
                for index in 0..if invalid { 1 } else { 3 } {
                    let (socket, _) = listener.accept().unwrap();
                    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut reader = BufReader::new(socket);
                    let mut line = String::new(); reader.read_line(&mut line).unwrap();
                    assert!(line.starts_with(["GET /api/auth/me ", "GET /api/usage-summary ", "POST /api/dashboard/get-sand-usage-status "][index]));
                    let mut found = false;
                    loop {
                        let mut header = String::new(); reader.read_line(&mut header).unwrap();
                        if header == "\r\n" { break; }
                        if header.trim() == "cookie: WorkosCursorSessionToken=test-session" { found = true; }
                        assert!(!header.to_ascii_lowercase().starts_with("authorization:"));
                    }
                    assert!(found);
                    let body = if index == 0 { if invalid { r#"{"email":"test@example.com"}"# } else { r#"{"sub":"test-user","email":"test@example.com"}"# } }
                        else if index == 1 { r#"{"membershipType":"premium","individualUsage":{"plan":{"totalPercentUsed":100}}}"# }
                        else { r#"{"isUnified":true,"usagePercent":25}"# };
                    write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                }
            });
            let client = reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().unwrap();
            let result = fetch_cookie(&client, &format!("http://{address}"), "WorkosCursorSessionToken=test-session");
            if invalid { assert!(result.is_err()); } else {
                let data = result.unwrap(); assert_eq!(data["windows"][0]["used"], 25.0);
                assert!(!data.to_string().contains("test-session"));
            }
            task.join().unwrap();
        }
    }
    #[test]
    fn unified_does_not_show_stale_monthly_allowance() {
        let data = parse(&json!({"individualUsage":{"plan":{"totalPercentUsed":100}}}), Some(&json!({"isUnified":true,"usagePercent":1.018,"nextResetTimestampUtc":"2026-11-15T12:00:00Z"})), "test@example.com", "test");
        assert_eq!(data["windows"].as_array().unwrap().len(), 1); assert_eq!(data["windows"][0]["used"], 1.018); assert_eq!(data["windows"][0]["kind"], "shared");
    }
    #[test]
    fn unknown_is_not_zero_or_assumed_shared() {
        let data = parse(&json!({}), None, "test@example.com", "test");
        assert!(data["unified"].is_null()); assert_eq!(data["windows"], json!([]));
        let data = parse(&json!({}), Some(&json!({"isUnified":true,"usagePercent":101})), "test@example.com", "test");
        assert!(data["windows"][0]["used"].is_null());
    }
}
