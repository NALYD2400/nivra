//! Experimental GitHub device authorization and Copilot token exchange.
use std::{sync::{Mutex,OnceLock},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use serde::Serialize;
use zeroize::Zeroizing;

const CLIENT:&str="Iv1.b507a08c87ecfe98";
const BINDING:&str="https://github.com/login/oauth/access_token";
const DEVICE:&str="https://github.com/login/device/code";
const TOKEN:&str="https://github.com/login/oauth/access_token";
const COPILOT:&str="https://api.github.com/copilot_internal/v2/token";
const USAGE:&str="https://api.github.com/copilot_internal/user";
struct Pending{code:Zeroizing<String>,deadline:Instant,interval:u64,cancel:tokio::sync::watch::Receiver<bool>}
static PENDING:OnceLock<Mutex<Option<Pending>>>=OnceLock::new();
static CANCEL:OnceLock<Mutex<Option<tokio::sync::watch::Sender<bool>>>>=OnceLock::new();
struct Cached{token:Zeroizing<String>,base:reqwest::Url,until:Instant}
static CACHE:OnceLock<tokio::sync::Mutex<std::collections::BTreeMap<String,Cached>>>=OnceLock::new();
#[derive(Serialize)] pub struct DevicePrompt{user_code:String,verification_uri:String}

fn client()->Result<reqwest::Client,String>{reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).user_agent("Nivra/0.1.0").build().map_err(|_|"Connexion GitHub indisponible.".into())}
fn form(values:&[(&str,&str)])->Zeroizing<String>{let mut url=reqwest::Url::parse("https://example.invalid/").unwrap();{let mut query=url.query_pairs_mut();for (k,v) in values{query.append_pair(k,v);}}Zeroizing::new(url.query().unwrap_or("").to_owned())}
async fn read(mut response:reqwest::Response)->Result<(u16,serde_json::Value),String>{let status=response.status().as_u16();let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse GitHub interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse GitHub trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}let value=serde_json::from_slice(&bytes).map_err(|_|"Réponse GitHub illisible.")?;Ok((status,value))}
async fn post(client:&reqwest::Client,url:reqwest::Url,body:&str)->Result<(u16,serde_json::Value),String>{read(client.post(url).header("accept","application/json").header("content-type","application/x-www-form-urlencoded").body(body.to_owned()).send().await.map_err(|_|"Impossible de joindre GitHub.")?).await}
fn bounded(value:Option<&serde_json::Value>,max:usize)->Option<&str>{value?.as_str().filter(|v|!v.is_empty()&&v.len()<=max&&!v.chars().any(char::is_control))}
fn parse_device(value:&serde_json::Value)->Result<(Pending,DevicePrompt,tokio::sync::watch::Sender<bool>),String>{
 let code=bounded(value.get("device_code"),2048).ok_or("Code GitHub absent.")?;let user=bounded(value.get("user_code"),64).filter(|v|v.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')).ok_or("Code utilisateur GitHub invalide.")?;
 let mut uri=reqwest::Url::parse("https://github.com/login/device").unwrap();uri.query_pairs_mut().append_pair("user_code",user);
 let expires=value.get("expires_in").and_then(|v|v.as_u64()).unwrap_or(900).clamp(1,900);let interval=value.get("interval").and_then(|v|v.as_u64()).unwrap_or(5).max(1);if interval>120{return Err("Délai GitHub trop long ; réessaie plus tard.".into())}let(sender,cancel)=tokio::sync::watch::channel(false);
 Ok((Pending{code:Zeroizing::new(code.into()),deadline:Instant::now()+Duration::from_secs(expires),interval,cancel},DevicePrompt{user_code:user.into(),verification_uri:uri.to_string()},sender))
}
fn api_base(raw:Option<&str>)->Result<reqwest::Url,String>{
 let raw=raw.unwrap_or("https://api.githubcopilot.com");let url=reqwest::Url::parse(raw).map_err(|_|"Adresse Copilot invalide.")?;let host=url.host_str().ok_or("Adresse Copilot invalide.")?;
 if url.scheme()!="https"||!url.username().is_empty()||url.password().is_some()||url.port().is_some()||!(host=="api.githubcopilot.com"||host.ends_with(".githubcopilot.com")){return Err("Adresse Copilot inattendue.".into())}
 reqwest::Url::parse(&format!("https://{host}/")).map_err(|_|"Adresse Copilot invalide.".into())
}
fn copilot_payload(value:&serde_json::Value)->Result<(Zeroizing<String>,reqwest::Url,u64),String>{
 let token=bounded(value.get("token"),8192).ok_or("Jeton Copilot absent.")?;
 let base=api_base(value.get("endpoints").and_then(|v|v.get("api")).and_then(|v|v.as_str()))?;
 let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|"Horloge système invalide.")?.as_secs();
 let seconds=value.get("expires_at").and_then(|v|v.as_u64()).map(|t|t.saturating_sub(now)).or_else(||value.get("refresh_in").and_then(|v|v.as_u64())).unwrap_or(1500).min(86400);
 if seconds==0{return Err("Jeton Copilot expiré.".into())}Ok((Zeroizing::new(token.into()),base,seconds))
}
async fn exchange(client:&reqwest::Client,url:reqwest::Url,github:&str)->Result<(Zeroizing<String>,reqwest::Url,u64),String>{
 let response=client.get(url).header("accept","application/json").header("editor-version","Nivra/0.1.0").header("editor-plugin-version","Nivra/0.1.0").header("copilot-integration-id","vscode-chat").header("authorization",format!("token {github}")).send().await.map_err(|_|"Impossible de joindre GitHub Copilot.")?;
 let(status,value)=read(response).await?;if !(200..300).contains(&status){return Err(format!("Accès Copilot refusé (HTTP {status}). Vérifie ton abonnement."))}copilot_payload(&value)
}
#[tauri::command] pub async fn copilot_login_start()->Result<DevicePrompt,String>{
 let (status,value)=post(&client()?,DEVICE.parse().unwrap(),&form(&[("client_id",CLIENT),("scope","read:user")])).await?;if !(200..300).contains(&status){return Err(format!("Connexion GitHub refusée (HTTP {status})."))}let(pending,prompt,sender)=parse_device(&value)?;
 {let mut slot=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion GitHub indisponible.")?;if let Some(previous)=slot.replace(sender){let _=previous.send(true);}}
 *PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion GitHub indisponible.")?=Some(pending);Ok(prompt)
}
#[tauri::command] pub fn copilot_login_cancel()->Result<(),String>{if let Some(sender)=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion GitHub indisponible.")?.take(){let _=sender.send(true);}*PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion GitHub indisponible.")?=None;Ok(())}
async fn poll(client:&reqwest::Client,url:reqwest::Url,pending:&mut Pending)->Result<Zeroizing<String>,String>{
 let mut interval=pending.interval;loop{
  if *pending.cancel.borrow(){return Err("Connexion GitHub annulée.".into())}let remaining=pending.deadline.saturating_duration_since(Instant::now());if remaining.is_zero(){return Err("Le code GitHub a expiré.".into())}
  tokio::select!{_ = tokio::time::sleep(Duration::from_secs(interval).min(remaining))=>{},_ = pending.cancel.changed()=>return Err("Connexion GitHub annulée.".into())};
  if Instant::now()>=pending.deadline{return Err("Le code GitHub a expiré.".into())}
  let payload=form(&[("client_id",CLIENT),("device_code",&pending.code),("grant_type","urn:ietf:params:oauth:grant-type:device_code")]);
  let(status,value)=tokio::select!{result=post(client,url.clone(),&payload)=>result?,_ = pending.cancel.changed()=>return Err("Connexion GitHub annulée.".into())};
  if let Some(access)=bounded(value.get("access_token"),8192){if !(200..300).contains(&status){return Err("Connexion GitHub refusée.".into())}let refresh=bounded(value.get("refresh_token"),8192);return Ok(Zeroizing::new(serde_json::json!({"access":access,"refresh":refresh}).to_string()))}
  match value.get("error").and_then(|v|v.as_str()){Some("authorization_pending")=>{},Some("slow_down")=>{interval=(interval+5).max(value.get("interval").and_then(|v|v.as_u64()).unwrap_or(0));if interval>120{return Err("Délai GitHub trop long ; réessaie plus tard.".into())}},Some("access_denied")=>return Err("Connexion GitHub refusée.".into()),Some("expired_token")=>return Err("Le code GitHub a expiré.".into()),_=>return Err(format!("Connexion GitHub impossible (HTTP {status})."))}
 }
}
#[tauri::command] pub async fn copilot_login_finish()->Result<String,String>{
 let mut pending=PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion GitHub indisponible.")?.take().ok_or("Aucune connexion GitHub en attente.")?;let client=client()?;let stored=poll(&client,TOKEN.parse().unwrap(),&mut pending).await?;
 let data:serde_json::Value=serde_json::from_str(&stored).map_err(|_|"Connexion GitHub invalide.")?;let access=bounded(data.get("access"),8192).ok_or("Jeton GitHub absent.")?;
 tokio::select!{result=exchange(&client,COPILOT.parse().unwrap(),access)=>{result?;},_ = pending.cancel.changed()=>return Err("Connexion GitHub annulée.".into())};
 if *pending.cancel.borrow(){return Err("Connexion GitHub annulée.".into())}crate::vault::gateway_remember_key(BINDING.into(),stored.to_string(),false,false)
}
pub(crate) fn validate(reference:&str)->Result<(),String>{let _=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);Ok(())}
pub(crate) async fn access(reference:&str)->Result<(Zeroizing<String>,reqwest::Url),String>{
 let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;let stored=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);cache.retain(|_,v|v.until>Instant::now());if let Some(item)=cache.get(reference){return Ok((Zeroizing::new(item.token.to_string()),item.base.clone()))}
 let data:serde_json::Value=serde_json::from_str(&stored).map_err(|_|"Connexion GitHub enregistrée invalide.")?;let mut github=Zeroizing::new(bounded(data.get("access"),8192).ok_or("Jeton GitHub absent.")?.to_owned());let client=client()?;
 // Expiring GitHub grants must be renewed before requesting the Copilot session.
 if let Some(refresh)=bounded(data.get("refresh"),8192){let(status,value)=post(&client,TOKEN.parse().unwrap(),&form(&[("client_id",CLIENT),("grant_type","refresh_token"),("refresh_token",refresh)])).await?;if !(200..300).contains(&status){return Err(format!("Renouvellement GitHub refusé (HTTP {status})."))}let new_access=bounded(value.get("access_token"),8192).ok_or("Jeton GitHub absent.")?;let new_refresh=bounded(value.get("refresh_token"),8192).unwrap_or(refresh);github=Zeroizing::new(new_access.into());let updated=Zeroizing::new(serde_json::json!({"access":new_access,"refresh":new_refresh}).to_string());crate::vault::replace_key(reference,BINDING,false,false,&updated)?;}
 let(token,base,seconds)=exchange(&client,COPILOT.parse().unwrap(),&github).await?;if seconds>120{if cache.len()>=32{if let Some(k)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&k);}}cache.insert(reference.into(),Cached{token:Zeroizing::new(token.to_string()),base:base.clone(),until:Instant::now()+Duration::from_secs(seconds-120)});}Ok((token,base))
}

#[derive(Serialize,Debug,PartialEq)]
pub struct QuotaWindow{kind:String,entitlement:Option<f64>,remaining:Option<f64>,percent_remaining:Option<f64>,unlimited:Option<bool>,overage_count:Option<f64>,overage_permitted:Option<bool>}
#[derive(Serialize,Debug,PartialEq)]
pub struct QuotaUsage{reset:Option<String>,windows:Vec<QuotaWindow>}
fn parse_usage(value:&serde_json::Value)->Result<QuotaUsage,String>{
 let object=value.as_object().ok_or("Réponse des quotas Copilot illisible.")?;
 let valid_date=|v:&&str|v.len()<=40&&!v.is_empty()&&v.bytes().all(|c|c.is_ascii_digit()||matches!(c,b'-'|b':'|b'T'|b'Z'|b'.'|b'+'));
 let reset=object.get("quota_reset_date_utc").and_then(|v|v.as_str()).filter(valid_date).or_else(||object.get("quota_reset_date").and_then(|v|v.as_str()).filter(valid_date)).map(str::to_owned);
 let mut windows=Vec::new();
 if let Some(snapshots)=object.get("quota_snapshots").and_then(|v|v.as_object()){
  for kind in ["premium_interactions","chat","completions"]{if let Some(row)=snapshots.get(kind).and_then(|v|v.as_object()){
   let number=|field:&str|row.get(field).and_then(|v|v.as_f64()).filter(|v|v.is_finite()&&*v>=0.0);
   let percent=number("percent_remaining").filter(|v|*v<=100.0);
   let window=QuotaWindow{kind:kind.into(),entitlement:number("entitlement"),remaining:number("remaining").or_else(||number("quota_remaining")),percent_remaining:percent,unlimited:row.get("unlimited").and_then(|v|v.as_bool()),overage_count:number("overage_count"),overage_permitted:row.get("overage_permitted").and_then(|v|v.as_bool())};
   if window.entitlement.is_some()||window.remaining.is_some()||window.percent_remaining.is_some()||window.unlimited.is_some()||window.overage_count.is_some()||window.overage_permitted.is_some(){windows.push(window)}
  }}
 }
 Ok(QuotaUsage{reset,windows})
}
async fn fetch_usage(client:&reqwest::Client,url:reqwest::Url,github:&str)->Result<QuotaUsage,String>{
 let response=client.get(url).header("accept","application/vnd.github+json").header("x-github-api-version","2022-11-28").header("editor-version","Nivra/0.1.0").header("editor-plugin-version","Nivra/0.1.0").header("copilot-integration-id","vscode-chat").header("authorization",format!("token {github}")).send().await.map_err(|_|"Impossible de lire les quotas GitHub Copilot.")?;
 let(status,value)=read(response).await?;if !(200..300).contains(&status){return Err(format!("Lecture des quotas Copilot refusée (HTTP {status})."))}parse_usage(&value)
}
#[tauri::command] pub async fn copilot_usage(reference:String)->Result<QuotaUsage,String>{
 // Refresh expiring GitHub grants under the same lock as the gateway before reading them.
 let _=access(&reference).await?;
 let _guard=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;
 let stored=Zeroizing::new(crate::vault::load_key(&reference,BINDING,false,false)?);let data:serde_json::Value=serde_json::from_str(&stored).map_err(|_|"Connexion GitHub enregistrée invalide.")?;
 let github=Zeroizing::new(bounded(data.get("access"),8192).ok_or("Jeton GitHub absent.")?.to_owned());fetch_usage(&client()?,USAGE.parse().unwrap(),&github).await
}
#[cfg(test)] mod tests{
 use super::*;use serde_json::json;
 #[test] fn device_url_is_built_locally(){let value=json!({"device_code":"secret","user_code":"ABCD-1234","verification_uri":"https://evil.example"});let(_,prompt,_)=parse_device(&value).unwrap();assert_eq!(prompt.verification_uri,"https://github.com/login/device?user_code=ABCD-1234");assert!(parse_device(&json!({"device_code":"x","user_code":"bad&code"})).is_err());}
 #[test] fn api_hosts_are_validated_and_normalized(){assert_eq!(api_base(Some("https://foo.githubcopilot.com/path?q=1")).unwrap().as_str(),"https://foo.githubcopilot.com/");for url in ["http://api.githubcopilot.com/","https://githubcopilot.com.evil.example/","https://user@api.githubcopilot.com/","https://api.githubcopilot.com:8443/"]{assert!(api_base(Some(url)).is_err());}}
 #[test] fn parses_session_only_with_valid_token_and_expiry(){assert!(copilot_payload(&json!({"token":"synthetic","refresh_in":3600})).is_ok());assert!(copilot_payload(&json!({"token":""})).is_err());assert!(copilot_payload(&json!({"token":"a","expires_at":1})).is_err());}
 #[test] fn quota_preserves_zero_and_absence_without_leaking_other_fields(){
  let value=json!({"quota_reset_date":"2026-12-01","email":"private@example.com","quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":0,"percent_remaining":0,"unlimited":false},"chat":{"unlimited":true},"completions":{"remaining":-1,"percent_remaining":101}}});let usage=parse_usage(&value).unwrap();assert_eq!(usage.reset.as_deref(),Some("2026-12-01"));assert_eq!(usage.windows.len(),2);assert_eq!(usage.windows[0].remaining,Some(0.0));assert_eq!(usage.windows[0].percent_remaining,Some(0.0));assert_eq!(usage.windows[1].unlimited,Some(true));assert!(!serde_json::to_string(&usage).unwrap().contains("private"));
  let empty=parse_usage(&json!({"quota_snapshots":null})).unwrap();assert!(empty.windows.is_empty());assert!(empty.reset.is_none());assert!(parse_usage(&json!(null)).is_err());
  let aliases=parse_usage(&json!({"quota_reset_date_utc":"2026-12-01T00:00:00Z","quota_snapshots":{"premium_interactions":{"remaining":0,"quota_remaining":9,"overage_count":2,"overage_permitted":true},"chat":{"quota_remaining":4}}})).unwrap();assert_eq!(aliases.reset.as_deref(),Some("2026-12-01T00:00:00Z"));assert_eq!(aliases.windows[0].remaining,Some(0.0));assert_eq!(aliases.windows[0].overage_count,Some(2.0));assert_eq!(aliases.windows[1].remaining,Some(4.0));
 }
 #[tokio::test] async fn exchanges_github_token_against_simulated_copilot(){
  use std::{sync::Arc,convert::Infallible};use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=Arc::new(Mutex::new(Some(tx)));let svc=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let auth=request.headers().get("authorization").unwrap().to_str().unwrap().to_owned();let path=request.uri().path().to_owned();tx.lock().unwrap().take().unwrap().send((auth,path)).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"token":"session","refresh_in":3600,"endpoints":{"api":"https://regional.githubcopilot.com"}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),svc).await;});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let(token,base,seconds)=exchange(&client,format!("http://{addr}/copilot_internal/v2/token").parse().unwrap(),"synthetic-github").await.unwrap();assert_eq!(token.as_str(),"session");assert_eq!(base.as_str(),"https://regional.githubcopilot.com/");assert_eq!(seconds,3600);let(auth,path)=rx.await.unwrap();assert_eq!(auth,"token synthetic-github");assert_eq!(path,"/copilot_internal/v2/token");
 }
 #[tokio::test] async fn reads_quota_using_github_token_from_simulated_service(){
  use std::{sync::Arc,convert::Infallible};use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=Arc::new(Mutex::new(Some(tx)));let svc=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let path=request.uri().path().to_owned();tx.lock().unwrap().take().unwrap().send((auth,path)).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"quota_reset_date":"2026-12-01","quota_snapshots":{"premium_interactions":{"remaining":23,"percent_remaining":46}}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),svc).await;});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let usage=fetch_usage(&client,format!("http://{addr}/copilot_internal/user").parse().unwrap(),"synthetic-github").await.unwrap();assert_eq!(usage.windows[0].remaining,Some(23.0));let(auth,path)=rx.await.unwrap();assert_eq!(auth,"token synthetic-github");assert_eq!(path,"/copilot_internal/user");
 }
 #[tokio::test] async fn polling_handles_pending_then_authorized(){
  use std::convert::Infallible;use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
  tauri::async_runtime::spawn(async move{for step in 0..2{let(socket,_)=listener.accept().await.unwrap();let svc=service_fn(move|_:Request<Incoming>|async move{let body=if step==0{br#"{"error":"authorization_pending"}"#.as_slice()}else{br#"{"access_token":"synthetic-github"}"#.as_slice()};Ok::<_,Infallible>(Response::new(Full::new(Bytes::copy_from_slice(body))))});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),svc).await;}});
  let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let(_sender,cancel)=tokio::sync::watch::channel(false);let mut pending=Pending{code:Zeroizing::new("device-code".into()),deadline:Instant::now()+Duration::from_secs(5),interval:0,cancel};let result=poll(&client,format!("http://{addr}/token").parse().unwrap(),&mut pending).await.unwrap();let data:serde_json::Value=serde_json::from_str(&result).unwrap();assert_eq!(data["access"],"synthetic-github");assert!(data["refresh"].is_null());
 }
}
