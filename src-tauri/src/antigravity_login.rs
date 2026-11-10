//! Browser login and account verification for Google Antigravity.
use serde::Serialize;
use std::{sync::{Arc,atomic::{AtomicBool,Ordering}},time::Duration};
use zeroize::Zeroizing;

// Desktop OAuth client configuration is injected at build time, outside version control.
fn require_client_configuration()->Result<(),String>{if CLIENT_ID.is_empty()||CLIENT_SECRET.is_empty(){Err("Connexion Antigravity indisponible : identifiants OAuth absents de cette compilation.".into())}else{Ok(())}}
const CLIENT_ID:&str=match option_env!("NIVRA_ANTIGRAVITY_CLIENT_ID"){Some(value)=>value,None=>""};
const CLIENT_SECRET:&str=match option_env!("NIVRA_ANTIGRAVITY_CLIENT_SECRET"){Some(value)=>value,None=>""};
const REDIRECT:&str="http://127.0.0.1:51121/callback";
const VAULT_BINDING:&str="https://oauth2.googleapis.com/token";
static ACTIVE:AtomicBool=AtomicBool::new(false);
static CANCEL:std::sync::OnceLock<tokio::sync::watch::Sender<bool>>=std::sync::OnceLock::new();
#[tauri::command]pub fn antigravity_gateway_cancel(){if let Some(sender)=CANCEL.get(){sender.send_replace(true);}}
struct Guard;
impl Drop for Guard{fn drop(&mut self){ACTIVE.store(false,Ordering::Release)}}
fn google_callback_result(uri:&hyper::Uri,path:&str,state:&str)->Option<Result<String,String>>{
    if uri.path()!=path{return None}let query=uri.query()?;if query.len()>4096{return None}let url=reqwest::Url::parse(&format!("http://127.0.0.1/?{query}")).ok()?;let fields:Vec<_>=url.query_pairs().collect();
    let states:Vec<_>=fields.iter().filter(|(key,_)|key=="state").collect();let codes:Vec<_>=fields.iter().filter(|(key,_)|key=="code").collect();let errors:Vec<_>=fields.iter().filter(|(key,_)|key=="error").collect();
    if states.len()!=1||states[0].1!=state{return None}if codes.len()==1&&errors.is_empty()&&!codes[0].1.is_empty()&&codes[0].1.len()<=2048&&!codes[0].1.chars().any(char::is_control){return Some(Ok(codes[0].1.to_string()))}
    if codes.is_empty()&&errors.len()==1&&errors[0].1=="access_denied"{return Some(Err("La connexion Google a été refusée ou annulée.".into()))}None
}
async fn google_callback(request:hyper::Request<hyper::body::Incoming>,path:Arc<String>,state:Arc<String>,sender:Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<String,String>>>>>) -> Result<hyper::Response<http_body_util::Full<bytes::Bytes>>,std::convert::Infallible>{
    let result=if request.method()==hyper::Method::GET{google_callback_result(request.uri(),&path,&state)}else{None};let accepted=result.is_some();let denied=result.as_ref().is_some_and(Result::is_err);
    if let Some(result)=result{if let Ok(mut sender)=sender.lock(){if let Some(sender)=sender.take(){let _=sender.send(result);}}}
    let body=if denied{"Connexion annulée. Tu peux revenir dans Nivra."}else if accepted{"Connexion reçue. Tu peux revenir dans Nivra."}else{"Cette demande de connexion est invalide."};
    Ok(hyper::Response::builder().status(if accepted{hyper::StatusCode::OK}else{hyper::StatusCode::BAD_REQUEST}).header("content-type","text/plain; charset=utf-8").header("cache-control","no-store").header("referrer-policy","no-referrer").body(http_body_util::Full::new(bytes::Bytes::from(body))).unwrap())
}

#[derive(Serialize)]
pub struct Account { id:String,email:String,project:String,credential_ref:String,project_error:Option<String> }
#[derive(Serialize)]
pub struct AccountStatus { email:String,project:String,models:Option<Vec<ModelInfo>>,quota:Option<Vec<QuotaWindow>>,models_error:Option<String>,quota_error:Option<String> }
#[derive(Serialize,Debug,PartialEq)]
pub struct ModelInfo { id:String,label:String,remaining:Option<f64>,reset:Option<String> }
#[derive(Serialize,Debug,PartialEq)]
pub struct QuotaWindow { group:String,window:String,remaining:f64,reset:Option<String> }

fn parse_quota(value:&serde_json::Value)->Result<Vec<QuotaWindow>,String>{
    let groups=value.get("groups").or_else(||value.pointer("/response/groups")).and_then(|v|v.as_array()).filter(|v|v.len()<=30).ok_or("Quotas Google illisibles.")?;
    let mut windows=Vec::new();
    for group in groups {
        let Some(label)=group.get("displayName").and_then(|v|v.as_str()).filter(|s|!s.trim().is_empty()&&s.len()<=150&&!s.chars().any(char::is_control)) else{continue};
        let Some(buckets)=group.get("buckets").and_then(|v|v.as_array()).filter(|v|v.len()<=50) else{continue};
        for bucket in buckets {
            let Some(window)=bucket.get("window").and_then(|v|v.as_str()).filter(|s|matches!(*s,"5h"|"weekly")) else{continue};
            let Some(remaining)=bucket.get("remainingFraction").and_then(|v|v.as_f64()).filter(|v|v.is_finite()&&(0.0..=1.0).contains(v)) else{continue};
            let reset=bucket.get("resetTime").and_then(|v|v.as_str()).filter(|s|s.len()<=64&&!s.chars().any(char::is_control)).map(str::to_owned);
            windows.push(QuotaWindow{group:label.to_owned(),window:window.to_owned(),remaining,reset});
        }
    }
    Ok(windows)
}
async fn fetch_quota(client:&reqwest::Client,url:reqwest::Url,token:&str)->Result<Vec<QuotaWindow>,String>{
    tokio::time::timeout(Duration::from_secs(12),async{
        let mut response=client.post(url).bearer_auth(token).header("content-type","application/json").body("{}").send().await.map_err(|_|"Impossible de lire les quotas Antigravity.")?;
        if !response.status().is_success(){return Err(format!("Lecture des quotas refusée (HTTP {}).",response.status().as_u16()))}
        let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture des quotas interrompue.")?{if bytes.len()+chunk.len()>1024*1024{return Err("Quotas trop volumineux.".into())}bytes.extend_from_slice(&chunk)}
        let value=serde_json::from_slice(&bytes).map_err(|_|"Quotas illisibles.")?;parse_quota(&value)
    }).await.map_err(|_|"Lecture des quotas expirée.".to_string())?
}

fn parse_models(value:&serde_json::Value)->Result<Vec<ModelInfo>,String>{
    let models=value.get("models").and_then(|v|v.as_object()).filter(|v|v.len()<=1000).ok_or("Liste des modèles Google illisible.")?;
    let mut result=Vec::new();
    for (id,model) in models {
        if id.is_empty()||id.len()>200||id.chars().any(|c|c.is_control()||c.is_whitespace()){continue}
        let label=model.get("displayName").or_else(||model.get("label")).and_then(|v|v.as_str()).filter(|label|!label.trim().is_empty()&&label.len()<=300&&!label.chars().any(char::is_control)).unwrap_or(id);
        let quota=model.get("quotaInfo");
        let remaining=quota.and_then(|q|q.get("remainingFraction")).and_then(|v|v.as_f64()).filter(|v|v.is_finite()&&(0.0..=1.0).contains(v));
        let reset=quota.and_then(|q|q.get("resetTime")).and_then(|v|v.as_str()).filter(|s|s.len()<=64&&!s.chars().any(char::is_control)).map(str::to_owned);
        result.push(ModelInfo{id:id.clone(),label:label.to_owned(),remaining,reset});
    }
    Ok(result)
}
async fn fetch_models(client:&reqwest::Client,url:reqwest::Url,token:&str,project:&str)->Result<Vec<ModelInfo>,String>{
    tokio::time::timeout(Duration::from_secs(15),async{
        let mut response=client.post(url).bearer_auth(token).header("content-type","application/json").body(serde_json::json!({"project":project}).to_string()).send().await.map_err(|_|"Impossible de lire les modèles Antigravity.")?;
        if !response.status().is_success(){return Err(format!("Lecture des modèles refusée (HTTP {}).",response.status().as_u16()))}
        let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture des modèles interrompue.")?{if bytes.len()+chunk.len()>2*1024*1024{return Err("Liste des modèles trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
        let data=serde_json::from_slice(&bytes).map_err(|_|"Liste des modèles illisible.")?;parse_models(&data)
    }).await.map_err(|_|"Lecture des modèles expirée.".to_string())?
}
static REFRESH_LOCK:std::sync::OnceLock<tokio::sync::Mutex<()>>=std::sync::OnceLock::new();

pub(crate) async fn selected_account(credential_ref:&str,account_id:&str)->Result<(crate::antigravity_tokens::Tokens,String),String>{
    let _lock=REFRESH_LOCK.get_or_init(||tokio::sync::Mutex::new(())).lock().await;
    let old=Zeroizing::new(crate::vault::load_key(credential_ref,VAULT_BINDING,false,false)?);
    require_client_configuration()?;
    let tokens=crate::antigravity_tokens::refresh(CLIENT_ID,CLIENT_SECRET,&old).await?;
    if tokens.refresh.as_str()!=old.as_str(){crate::vault::replace_key(credential_ref,VAULT_BINDING,false,false,&tokens.refresh)?;}
    let identity=crate::antigravity_tokens::identity(&tokens.access).await?;
    if identity.id!=account_id{return Err("L’identité Google ne correspond pas au compte enregistré. Reconnecte ce compte.".into())}
    Ok((tokens,identity.email))
}

pub(crate) async fn gateway_catalog(reference:&str,id:&str)->Result<Vec<String>,String>{
    let (tokens,_)=selected_account(reference,id).await?;let project=crate::antigravity_project::discover(&tokens.access).await?;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    let models=fetch_models(&client,reqwest::Url::parse("https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels").unwrap(),&tokens.access,&project).await?;
    Ok(models.into_iter().map(|model|model.id).collect())
}

#[tauri::command]
pub async fn antigravity_gateway_check(credential_ref:String,account_id:String)->Result<AccountStatus,String>{
    let (tokens,email)=selected_account(&credential_ref,&account_id).await?;
    let project=crate::antigravity_project::discover(&tokens.access).await?;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    let (models,quota)=tokio::join!(
        fetch_models(&client,reqwest::Url::parse("https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels").unwrap(),&tokens.access,&project),
        fetch_quota(&client,reqwest::Url::parse("https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary").unwrap(),&tokens.access)
    );
    Ok(account_status(email,project,models,quota))
}
fn account_status(email:String,project:String,models:Result<Vec<ModelInfo>,String>,quota:Result<Vec<QuotaWindow>,String>)->AccountStatus{
    let(models,models_error)=match models{Ok(value)=>(Some(value),None),Err(error)=>(None,Some(error))};
    let(quota,quota_error)=match quota{Ok(value)=>(Some(value),None),Err(error)=>(None,Some(error))};
    AccountStatus{email,project,models,quota,models_error,quota_error}
}

fn response_text(value:&serde_json::Value)->Result<String,String>{
    let candidates=value.pointer("/response/candidates").and_then(|v|v.as_array()).filter(|v|v.len()==1).ok_or("Réponse Antigravity indisponible.")?;
    let candidate=&candidates[0];if !matches!(candidate.get("finishReason").and_then(|v|v.as_str()),Some("STOP"|"MAX_TOKENS")){return Err("La réponse Antigravity est incomplète ou bloquée.".into())}
    let parts=candidate.pointer("/content/parts").and_then(|v|v.as_array()).ok_or("Réponse Antigravity illisible.")?;let mut text=String::new();
    for part in parts {if part.get("thought").and_then(|v|v.as_bool())==Some(true){continue}let value=part.get("text").and_then(|v|v.as_str()).ok_or("Réponse non textuelle non prise en charge.")?;text.push_str(value);if text.len()>1024*1024{return Err("Réponse Antigravity trop volumineuse.".into())}}
    if text.is_empty(){return Err("Réponse Antigravity vide.".into())}Ok(text)
}
#[derive(Debug)]
pub(crate) struct GeneratedAnswer { pub text:String, pub input_tokens:Option<u64>, pub output_tokens:Option<u64>, pub truncated:bool, pub reply:Option<crate::google_request::GoogleReply> }
pub(crate) fn token_usage(value:&serde_json::Value)->Option<(u64,u64)>{
    fn optional(value:&serde_json::Value,key:&str)->Option<u64>{match value.get(key){None|Some(serde_json::Value::Null)=>Some(0),Some(value)=>value.as_u64()}}
    let input=value.get("promptTokenCount")?.as_u64()?.checked_add(optional(value,"toolUsePromptTokenCount")?)?;
    let output=value.get("candidatesTokenCount")?.as_u64()?.checked_add(optional(value,"thoughtsTokenCount")?)?;
    Some((input,output))
}
fn generated_answer(value:&serde_json::Value)->Result<GeneratedAnswer,String>{
    let has_calls=value.pointer("/response/candidates/0/content/parts").and_then(|v|v.as_array()).is_some_and(|parts|parts.iter().any(|part|part.get("functionCall").is_some()));
    let reply=if has_calls{Some(crate::google_request::response(value.get("response").ok_or("Réponse Antigravity absente.")?,"antigravity")?)}else{None};
    let text=if let Some(reply)=&reply{reply.chat.pointer("/choices/0/message/content").and_then(|v|v.as_str()).unwrap_or("").to_owned()}else{response_text(value)?};
    let usage=value.pointer("/response/usageMetadata");
    let counts=usage.and_then(token_usage);let input_tokens=counts.map(|v|v.0);let output_tokens=counts.map(|v|v.1);
    let truncated=value.pointer("/response/candidates/0/finishReason").and_then(|v|v.as_str())==Some("MAX_TOKENS");
    Ok(GeneratedAnswer{text,input_tokens,output_tokens,truncated,reply})
}
async fn generate(client:&reqwest::Client,url:reqwest::Url,token:&str,project:&str,model:&str,mut request:serde_json::Value,max_tokens:u32)->Result<GeneratedAnswer,String>{
    if !request.get("generationConfig").is_some_and(|v|v.is_object()){request["generationConfig"]=serde_json::json!({})}
    request["generationConfig"]["maxOutputTokens"]=serde_json::json!(max_tokens);
    let body=serde_json::json!({"project":project,"model":model,"userAgent":"antigravity","requestType":"agent","requestId":format!("agent-{}",uuid::Uuid::new_v4()),"request":request});
    tokio::time::timeout(Duration::from_secs(60),async{
        let mut response=client.post(url).bearer_auth(token).header("content-type","application/json").body(body.to_string()).send().await.map_err(|_|"Impossible de joindre Antigravity.")?;
        if !response.status().is_success(){return Err(format!("Antigravity a refusé la requête (HTTP {}).",response.status().as_u16()))}
        let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse Antigravity interrompue.")?{if bytes.len()+chunk.len()>2*1024*1024{return Err("Réponse Antigravity trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
        let value=serde_json::from_slice(&bytes).map_err(|_|"Réponse Antigravity illisible.")?;generated_answer(&value)
    }).await.map_err(|_|"La réponse Antigravity a expiré.".to_string())?
}
#[tauri::command]
pub async fn antigravity_try_model(credential_ref:String,account_id:String,model:String,prompt:String)->Result<String,String>{
    if prompt.trim().is_empty()||prompt.len()>32000{return Err("Écris une demande de 1 à 32 000 caractères.".into())}
    let request=serde_json::json!({"contents":[{"role":"user","parts":[{"text":prompt}]}]});
    generate_for_account(credential_ref,account_id,model,request,2048).await.and_then(|answer|if answer.truncated||answer.reply.is_some(){Err("La réponse du modèle ne peut pas être affichée dans cet essai texte.".into())}else{Ok(answer.text)})
}

pub(crate) async fn generate_for_account(credential_ref:String,account_id:String,model:String,request:serde_json::Value,max_tokens:u32)->Result<GeneratedAnswer,String>{
    if !(1..=65536).contains(&max_tokens){return Err("La limite de sortie Antigravity doit être comprise entre 1 et 65536.".into())}
    let (tokens,_)=selected_account(&credential_ref,&account_id).await?;
    let project=crate::antigravity_project::discover(&tokens.access).await?;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    let models=fetch_models(&client,reqwest::Url::parse("https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels").unwrap(),&tokens.access,&project).await?;
    if !models.iter().any(|entry|entry.id==model){return Err("Ce modèle n’est pas dans la liste actuelle de ce compte.".into())}
    generate(&client,reqwest::Url::parse("https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent").unwrap(),&tokens.access,&project,&model,request,max_tokens).await
}

pub(crate) async fn stream_for_account(credential_ref:String,account_id:String,model:String,mut request:serde_json::Value,max_tokens:u32)->Result<reqwest::Response,String>{
    if !(1..=65536).contains(&max_tokens){return Err("Limite de sortie invalide.".into())}
    let (tokens,_)=selected_account(&credential_ref,&account_id).await?;
    let project=crate::antigravity_project::discover(&tokens.access).await?;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    let models=fetch_models(&client,reqwest::Url::parse("https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels").unwrap(),&tokens.access,&project).await?;
    if !models.iter().any(|entry|entry.id==model){return Err("Ce modèle n’est pas dans la liste actuelle de ce compte.".into())}
    if !request.get("generationConfig").is_some_and(|v|v.is_object()){request["generationConfig"]=serde_json::json!({})}
    request["generationConfig"]["maxOutputTokens"]=serde_json::json!(max_tokens);
    let body=serde_json::json!({"project":project,"model":model,"userAgent":"antigravity","requestType":"agent","requestId":format!("agent-{}",uuid::Uuid::new_v4()),"request":request});
    let response=tokio::time::timeout(Duration::from_secs(60),client.post("https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse").bearer_auth(tokens.access.as_str()).header("content-type","application/json").body(body.to_string()).send()).await.map_err(|_|"La connexion Antigravity a expiré.")?.map_err(|_|"Impossible de joindre Antigravity.")?;
    if !response.status().is_success(){return Err(format!("Antigravity a refusé la requête (HTTP {}).",response.status().as_u16()))}
    if !response.headers().get("content-type").and_then(|v|v.to_str().ok()).is_some_and(|v|v.to_ascii_lowercase().starts_with("text/event-stream")){return Err("Antigravity n'a pas renvoyé un flux SSE.".into())}
    Ok(response)
}

#[tauri::command]
pub async fn antigravity_gateway_login()->Result<Account,String>{
    require_client_configuration()?;
    if ACTIVE.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("Une connexion Antigravity est déjà en cours.".into())}let _guard=Guard;
    #[cfg(not(windows))] {return Err("Connexion Antigravity disponible uniquement sur Windows.".into())}
    #[cfg(windows)] {
        let sender=CANCEL.get_or_init(||tokio::sync::watch::channel(false).0);sender.send_replace(false);let mut cancel=sender.subscribe();
        tokio::select! {
        _=cancel.changed()=>Err("Connexion Google annulée.".into()),
        result=async {
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:51121").await.map_err(|_|"Le port de connexion Google 51121 est occupé. Ferme l'autre connexion et réessaie.")?;
        let verifier=Zeroizing::new(format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()));
        let state=Arc::new(format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()));let path=Arc::new("/callback".to_string());
        let mut auth=reqwest::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
        auth.query_pairs_mut().append_pair("response_type","code").append_pair("client_id",CLIENT_ID).append_pair("redirect_uri",REDIRECT)
            .append_pair("scope","https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs")
            .append_pair("code_challenge",&crate::openrouter_login::challenge(&verifier)).append_pair("code_challenge_method","S256").append_pair("access_type","offline").append_pair("prompt","consent select_account").append_pair("state",&state);
        crate::browser::open(auth.as_str())?;
        let(tx,mut rx)=tokio::sync::oneshot::channel();let sender=Arc::new(std::sync::Mutex::new(Some(tx)));
        let code=tokio::time::timeout(Duration::from_secs(180),async{
            let mut count=0;loop{tokio::select!{
                result=&mut rx=>break result.map_err(|_|"Connexion interrompue.".to_string())?,
                incoming=listener.accept()=>{let(socket,_)=incoming.map_err(|_|"Retour local interrompu.".to_string())?;count+=1;if count>32{return Err("Trop de demandes de connexion.".into())}let path=path.clone();let state=state.clone();let sender=sender.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|google_callback(request,path.clone(),state.clone(),sender.clone()));let _=tokio::time::timeout(Duration::from_secs(5),hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service)).await;});}
            }}
        }).await.map_err(|_|"La connexion Google a expiré.".to_string())??;
        let code=Zeroizing::new(code);let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
        let tokens=crate::antigravity_tokens::exchange(&client,reqwest::Url::parse("https://oauth2.googleapis.com/token").unwrap(),&[("grant_type","authorization_code"),("client_id",CLIENT_ID),("client_secret",CLIENT_SECRET),("code",&code),("redirect_uri",REDIRECT),("code_verifier",&verifier)],None).await?;
        let identity=crate::antigravity_tokens::identity(&tokens.access).await?;
        let(project,project_error)=match crate::antigravity_project::discover(&tokens.access).await{Ok(project)=>(project,None),Err(error)=>(String::new(),Some(error))};
        let credential_ref=crate::vault::gateway_remember_key(VAULT_BINDING.into(),tokens.refresh.to_string(),false,false)?;
        Ok(Account{id:identity.id,email:identity.email,project,credential_ref,project_error})
        }=>result,
        }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use bytes::Bytes;use http_body_util::{BodyExt,Full};use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;
    #[test]fn google_callback_requires_matching_state_and_reports_denial(){let good="/callback?code=abc&state=right".parse().unwrap();assert_eq!(google_callback_result(&good,"/callback","right").unwrap().unwrap(),"abc");let denied="/callback?error=access_denied&state=right".parse().unwrap();assert!(google_callback_result(&denied,"/callback","right").unwrap().is_err());for value in ["/callback?error=access_denied&state=wrong","/callback?error=access_denied&code=x&state=right","/callback?error=access_denied&state=right&state=right","/wrong?error=access_denied&state=right"]{assert!(google_callback_result(&value.parse().unwrap(),"/callback","right").is_none());}}
    #[tokio::test]async fn google_denial_returns_immediately_to_application(){let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(std::sync::Mutex::new(Some(tx)));tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|google_callback(request,Arc::new("/callback".into()),Arc::new("right".into()),sender.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.get(format!("http://{address}/callback?error=access_denied&state=right")).send().await.unwrap();assert_eq!(response.status(),hyper::StatusCode::OK);assert!(rx.await.unwrap().is_err());}
    #[test] fn model_catalog_discards_malformed_names_and_quota_values(){
        let models=parse_models(&serde_json::json!({"models":{"good-model":{"displayName":"Modèle","quotaInfo":{"remainingFraction":1}},"bad model":{},"other":{}}})).unwrap();
        assert_eq!(models,vec![ModelInfo{id:"good-model".into(),label:"Modèle".into(),remaining:Some(1.0),reset:None},ModelInfo{id:"other".into(),label:"other".into(),remaining:None,reset:None}]);
        assert!(parse_models(&serde_json::json!({"models":[]})).is_err());
        let models=parse_models(&serde_json::json!({"models":{"empty":{"quotaInfo":{}},"zero":{"quotaInfo":{"remainingFraction":0,"resetTime":"2030-01-01T00:00:00Z"}},"bad":{"quotaInfo":{"remainingFraction":-1}}}})).unwrap();assert_eq!(models.iter().find(|m|m.id=="zero").unwrap().remaining,Some(0.0));assert_eq!(models.iter().find(|m|m.id=="zero").unwrap().reset.as_deref(),Some("2030-01-01T00:00:00Z"));assert!(models.iter().find(|m|m.id=="empty").unwrap().remaining.is_none());assert!(models.iter().find(|m|m.id=="bad").unwrap().remaining.is_none());
    }
    #[test] fn quota_preserves_zero_and_omits_unknown_values(){
        let quota=parse_quota(&serde_json::json!({"groups":[{"displayName":"Gemini","buckets":[{"window":"5h","remainingFraction":0,"resetTime":"2030-01-01T00:00:00Z"},{"window":"weekly"},{"window":"weekly","remainingFraction":0.5}]}]})).unwrap();
        assert_eq!(quota.len(),2);assert_eq!(quota[0].remaining,0.0);assert_eq!(quota[1].remaining,0.5);assert!(parse_quota(&serde_json::json!({})).is_err());
        let wrapped=parse_quota(&serde_json::json!({"response":{"groups":[{"displayName":"Claude et GPT","buckets":[{"window":"weekly","remainingFraction":0.25},{"window":"5h"}]}]}})).unwrap();assert_eq!(wrapped.len(),1);assert_eq!(wrapped[0].group,"Claude et GPT");assert_eq!(wrapped[0].remaining,0.25);
    }
    #[test] fn account_status_distinguishes_empty_data_from_failed_reads(){let result=account_status("a@example.test".into(),"project".into(),Ok(vec![]),Err("Lecture refusée (HTTP 403).".into()));assert_eq!(result.models.unwrap().len(),0);assert!(result.models_error.is_none());assert!(result.quota.is_none());assert_eq!(result.quota_error.as_deref(),Some("Lecture refusée (HTTP 403)."));}
    #[test] fn generation_preserves_reported_usage_without_inventing_counts(){
        let mut value=serde_json::json!({"response":{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"Salut"}]}}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":3}}});
        let answer=generated_answer(&value).unwrap();assert_eq!(answer.input_tokens,Some(8));assert_eq!(answer.output_tokens,Some(3));
        value["response"]["usageMetadata"]["thoughtsTokenCount"]=serde_json::json!(4);value["response"]["usageMetadata"]["toolUsePromptTokenCount"]=serde_json::json!(2);let answer=generated_answer(&value).unwrap();assert_eq!(answer.input_tokens,Some(10));assert_eq!(answer.output_tokens,Some(7));
        value["response"]["usageMetadata"]=serde_json::Value::Null;let answer=generated_answer(&value).unwrap();assert_eq!(answer.input_tokens,None);assert_eq!(answer.output_tokens,None);
    }
    #[test] fn token_usage_rejects_invalid_or_overflowing_breakdowns(){assert_eq!(token_usage(&serde_json::json!({"promptTokenCount":8,"candidatesTokenCount":3,"thoughtsTokenCount":-1})),None);assert_eq!(token_usage(&serde_json::json!({"promptTokenCount":u64::MAX,"toolUsePromptTokenCount":1,"candidatesTokenCount":3})),None);assert_eq!(token_usage(&serde_json::json!({"promptTokenCount":8})),None);}
    #[test] fn generation_accepts_signed_function_calls_without_exposing_thoughts(){
        let raw=serde_json::json!({"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"raisonnement","thought":true},{"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"signature"}]},"finishReason":"STOP"}]}});
        let answer=generated_answer(&raw).unwrap();let reply=answer.reply.unwrap();assert_eq!(reply.chat["choices"][0]["finish_reason"],"tool_calls");assert!(reply.chat.to_string().find("raisonnement").is_none());assert_eq!(reply.model_content["parts"][1]["thoughtSignature"],"signature");
    }
    #[test] fn generation_rejects_incomplete_and_non_text_replies(){
        assert_eq!(response_text(&serde_json::json!({"response":{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"Bonjour"}]}}]}})).unwrap(),"Bonjour");
        let partial=generated_answer(&serde_json::json!({"response":{"candidates":[{"finishReason":"MAX_TOKENS","content":{"parts":[{"text":"partiel"}]}}]}})).unwrap();assert_eq!(partial.text,"partiel");assert!(partial.truncated);
        assert!(response_text(&serde_json::json!({"response":{"candidates":[{"finishReason":"SAFETY","content":{"parts":[{"text":"bloqué"}]}}]}})).is_err());
        assert!(response_text(&serde_json::json!({"response":{"candidates":[{"finishReason":"STOP","content":{"parts":[{"functionCall":{"name":"run"}}]}}]}})).is_err());
    }
    #[tokio::test] async fn model_lookup_uses_the_verified_account_and_project(){
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,body)).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"models":{"model-a":{"displayName":"Model A"}}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let models=fetch_models(&client,format!("http://{address}/models").parse().unwrap(),"account-a","project-a").await.unwrap();assert_eq!(models[0].id,"model-a");let(auth,body)=rx.await.unwrap();assert_eq!(auth,"Bearer account-a");assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["project"],"project-a");
    }
    #[tokio::test] async fn quota_lookup_uses_selected_account_token(){
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();sender.lock().unwrap().take().unwrap().send(auth).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"groups":[{"displayName":"Gemini","buckets":[{"window":"weekly","remainingFraction":0.25}]}]}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let quota=fetch_quota(&client,format!("http://{address}/quota").parse().unwrap(),"account-a").await.unwrap();assert_eq!(quota[0].remaining,0.25);assert_eq!(rx.await.unwrap(),"Bearer account-a");
    }
    #[tokio::test] async fn generation_sends_prompt_with_selected_account_project_and_model(){
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,body)).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"response":{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"Bonjour"}]}}]}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let answer=generate(&client,format!("http://{address}/generate").parse().unwrap(),"account-a","project-a","model-a",serde_json::json!({"generationConfig":{"temperature":0.4,"topP":0.8},"systemInstruction":{"parts":[{"text":"Guide"}]},"contents":[{"role":"user","parts":[{"text":"Salut"}]},{"role":"model","parts":[{"text":"Bonjour"}]},{"role":"user","parts":[{"text":"Suite"}]}]}),123).await.unwrap();assert_eq!(answer.text,"Bonjour");let(auth,body)=rx.await.unwrap();assert_eq!(auth,"Bearer account-a");let body:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["request"]["generationConfig"]["maxOutputTokens"],123);assert_eq!(body["request"]["generationConfig"]["temperature"],0.4);assert_eq!(body["request"]["generationConfig"]["topP"],0.8);assert_eq!(body["project"],"project-a");assert_eq!(body["model"],"model-a");assert_eq!(body["request"]["contents"][0]["parts"][0]["text"],"Salut");assert_eq!(body["request"]["systemInstruction"]["parts"][0]["text"],"Guide");assert_eq!(body["request"]["contents"][2]["parts"][0]["text"],"Suite");
    }
}
