//! Qwen account device authorization; refresh tokens are kept in the Windows vault.
use std::{sync::{Mutex,OnceLock},time::{Duration,Instant}};
use serde::Serialize;
use zeroize::Zeroizing;
const CLIENT:&str="f0304373b74a44d2b584a3fb70ca9e56";
const DEVICE:&str="https://chat.qwen.ai/api/v1/oauth2/device/code";
const TOKEN:&str="https://chat.qwen.ai/api/v1/oauth2/token";
const BINDING:&str="https://chat.qwen.ai/api/v1/oauth2/token";
const SCOPE:&str="openid profile email model.completion";
struct Pending{code:Zeroizing<String>,verifier:Zeroizing<String>,deadline:Instant,interval:u64,cancel:tokio::sync::watch::Receiver<bool>}
static PENDING:OnceLock<Mutex<Option<Pending>>>=OnceLock::new();
static CANCEL:OnceLock<Mutex<Option<tokio::sync::watch::Sender<bool>>>>=OnceLock::new();
struct Cached{access:Zeroizing<String>,until:Instant}
static CACHE:OnceLock<tokio::sync::Mutex<std::collections::BTreeMap<String,Cached>>>=OnceLock::new();
#[derive(Serialize)]pub struct Prompt{user_code:String,verification_uri:String}
fn bounded(value:Option<&serde_json::Value>,max:usize)->Option<&str>{value?.as_str().filter(|s|!s.is_empty()&&s.len()<=max&&!s.chars().any(char::is_control))}
fn client()->Result<reqwest::Client,String>{reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build().map_err(|_|"Connexion Qwen indisponible.".into())}
fn form(values:&[(&str,&str)])->Zeroizing<String>{let mut url=reqwest::Url::parse("https://example.invalid/").unwrap();{let mut pairs=url.query_pairs_mut();for(k,v)in values{pairs.append_pair(k,v);}}Zeroizing::new(url.query().unwrap_or("").to_owned())}
async fn post(client:&reqwest::Client,url:reqwest::Url,body:&str)->Result<(u16,serde_json::Value),String>{
 let mut response=client.post(url).header("accept","application/json").header("content-type","application/x-www-form-urlencoded").body(body.to_owned()).send().await.map_err(|_|"Impossible de joindre Qwen.")?;
 let status=response.status().as_u16();let mut data=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse Qwen interrompue.")?{if data.len()+chunk.len()>65536{return Err("Réponse Qwen trop volumineuse.".into())}data.extend_from_slice(&chunk)}
 Ok((status,serde_json::from_slice(&data).map_err(|_|"Réponse Qwen illisible.")?))
}
fn parse_device(value:&serde_json::Value,verifier:Zeroizing<String>)->Result<(Pending,Prompt,tokio::sync::watch::Sender<bool>),String>{
 let code=bounded(value.get("device_code"),2048).ok_or("Code Qwen absent.")?;let user=bounded(value.get("user_code"),64).ok_or("Code utilisateur Qwen absent.")?;
 let uri=bounded(value.get("verification_uri_complete"),2048).or_else(||bounded(value.get("verification_uri"),2048)).ok_or("Adresse Qwen absente.")?;let url=reqwest::Url::parse(uri).map_err(|_|"Adresse Qwen invalide.")?;
 if url.scheme()!="https"||url.host_str()!=Some("chat.qwen.ai")||!url.username().is_empty()||url.password().is_some()||url.port().is_some(){return Err("Adresse de connexion Qwen inattendue.".into())}
 let seconds=value.get("expires_in").and_then(|v|v.as_u64()).unwrap_or(600).clamp(1,900);let interval=value.get("interval").and_then(|v|v.as_u64()).unwrap_or(5).max(1);if interval>120{return Err("Délai Qwen trop long.".into())}
 let(sender,cancel)=tokio::sync::watch::channel(false);Ok((Pending{code:Zeroizing::new(code.into()),verifier,deadline:Instant::now()+Duration::from_secs(seconds),interval,cancel},Prompt{user_code:user.into(),verification_uri:url.to_string()},sender))
}
fn tokens(value:&serde_json::Value,old:Option<&str>)->Result<(Zeroizing<String>,Zeroizing<String>,u64),String>{
 if value.get("resource_url").is_some_and(|v|!v.is_null()&&v.as_str()!=Some("portal.qwen.ai")){return Err("Ce compte Qwen utilise une adresse non prise en charge.".into())}
 let access=bounded(value.get("access_token"),8192).ok_or("Jeton Qwen absent.")?;
 let refresh=match value.get("refresh_token"){None|Some(serde_json::Value::Null)=>old,Some(v)=>bounded(Some(v),8192)}.ok_or("Renouvellement Qwen absent.")?;
 let expiry=value.get("expires_in").and_then(|v|v.as_u64()).filter(|n|*n>0&&*n<=30*86400).ok_or("Durée Qwen invalide.")?;
 if value.get("token_type").and_then(|v|v.as_str()).is_some_and(|v|!v.eq_ignore_ascii_case("bearer")){return Err("Type de jeton Qwen invalide.".into())}
 Ok((Zeroizing::new(access.into()),Zeroizing::new(refresh.into()),expiry))
}
#[tauri::command]pub async fn qwen_login_start()->Result<Prompt,String>{
 let verifier=Zeroizing::new(format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()));let challenge=crate::openrouter_login::challenge(&verifier);
 let(status,value)=post(&client()?,DEVICE.parse().unwrap(),&form(&[("client_id",CLIENT),("scope",SCOPE),("code_challenge",&challenge),("code_challenge_method","S256")])).await?;
 if !(200..300).contains(&status){return Err(format!("Connexion Qwen refusée (HTTP {status})."))}let(pending,prompt,sender)=parse_device(&value,verifier)?;
 {let mut slot=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Qwen indisponible.")?;if let Some(previous)=slot.replace(sender){let _=previous.send(true);}}
 *PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Qwen indisponible.")?=Some(pending);Ok(prompt)
}
#[tauri::command]pub fn qwen_login_cancel()->Result<(),String>{if let Some(sender)=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Qwen indisponible.")?.take(){let _=sender.send(true);}*PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Qwen indisponible.")?=None;Ok(())}
async fn poll(client:&reqwest::Client,url:reqwest::Url,pending:&mut Pending)->Result<(Zeroizing<String>,Zeroizing<String>,u64),String>{
 let mut interval=pending.interval;loop{
  if *pending.cancel.borrow(){return Err("Connexion Qwen annulée.".into())}if Instant::now()>=pending.deadline{return Err("Le code Qwen a expiré.".into())}
  let payload=form(&[("grant_type","urn:ietf:params:oauth:grant-type:device_code"),("client_id",CLIENT),("device_code",&pending.code),("code_verifier",&pending.verifier)]);
  let(status,value)=tokio::select!{result=post(client,url.clone(),&payload)=>result?,_=pending.cancel.changed()=>return Err("Connexion Qwen annulée.".into())};
  if (200..300).contains(&status)&&value.get("access_token").and_then(|v|v.as_str()).is_some(){return tokens(&value,None)}
  match value.get("error").or_else(||value.get("status")).and_then(|v|v.as_str()){Some("authorization_pending"|"pending")=>{},Some("slow_down")=>{interval+=5;if interval>120{return Err("Délai Qwen trop long.".into())}},Some("access_denied")=>return Err("Connexion Qwen refusée.".into()),Some("expired_token")=>return Err("Le code Qwen a expiré.".into()),_=>return Err(format!("Connexion Qwen impossible (HTTP {status})."))}
  let remaining=pending.deadline.saturating_duration_since(Instant::now());tokio::select!{_=tokio::time::sleep(Duration::from_secs(interval).min(remaining))=>{},_=pending.cancel.changed()=>return Err("Connexion Qwen annulée.".into())};
 }
}
#[tauri::command]pub async fn qwen_login_finish()->Result<String,String>{
 let mut pending=PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Qwen indisponible.")?.take().ok_or("Aucune connexion Qwen en attente.")?;
 let(access,refresh,expires)=poll(&client()?,TOKEN.parse().unwrap(),&mut pending).await?;if *pending.cancel.borrow(){return Err("Connexion Qwen annulée.".into())}
 let reference=crate::vault::gateway_remember_key(BINDING.into(),refresh.to_string(),false,false)?;
 if expires>60{let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;if cache.len()>=32{if let Some(key)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&key);}}cache.insert(reference.clone(),Cached{access,until:Instant::now()+Duration::from_secs(expires-60)});}Ok(reference)
}
pub(crate) fn validate(reference:&str)->Result<(),String>{let _=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);Ok(())}
#[cfg(all(test,windows))]pub(crate) async fn synthetic_cached_session()->String{let reference=crate::vault::gateway_remember_key(BINDING.into(),"synthetic-refresh".into(),false,false).unwrap();CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await.insert(reference.clone(),Cached{access:Zeroizing::new("synthetic-qwen-access".into()),until:Instant::now()+Duration::from_secs(60)});reference}
pub(crate) async fn access(reference:&str)->Result<Zeroizing<String>,String>{
 let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;let old=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);cache.retain(|_,v|v.until>Instant::now());if let Some(v)=cache.get(reference){return Ok(Zeroizing::new(v.access.to_string()))}
 let(status,value)=post(&client()?,TOKEN.parse().unwrap(),&form(&[("grant_type","refresh_token"),("client_id",CLIENT),("refresh_token",&old)])).await?;if !(200..300).contains(&status){return Err(format!("Renouvellement Qwen refusé (HTTP {status})."))}let(access,refresh,expires)=tokens(&value,Some(&old))?;
 if refresh.as_str()!=old.as_str(){crate::vault::replace_key(reference,BINDING,false,false,&refresh)?}if expires>60{if cache.len()>=32{if let Some(key)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&key);}}cache.insert(reference.into(),Cached{access:Zeroizing::new(access.to_string()),until:Instant::now()+Duration::from_secs(expires-60)});}Ok(access)
}
#[cfg(test)]mod tests{use super::*;use serde_json::json;
 #[test]fn validates_device_and_token_responses(){let verifier=||Zeroizing::new("synthetic-verifier".to_owned());let good=json!({"device_code":"d","user_code":"ABCD","verification_uri":"https://chat.qwen.ai/api/v1/oauth2/device","interval":5});assert!(parse_device(&good,verifier()).is_ok());let mut bad=good.clone();bad["verification_uri"]=json!("https://chat.qwen.ai.evil.test/");assert!(parse_device(&bad,verifier()).is_err());bad=good;bad["interval"]=json!(121);assert!(parse_device(&bad,verifier()).is_err());assert!(tokens(&json!({"access_token":"a","refresh_token":"r","expires_in":3600,"token_type":"Bearer"}),None).is_ok());assert_eq!(tokens(&json!({"access_token":"a","expires_in":60}),Some("old")).unwrap().1.as_str(),"old");assert!(tokens(&json!({"access_token":"a","expires_in":60,"refresh_token":""}),Some("old")).is_err());}
 #[test]fn rejects_unrecognized_token_resource(){assert!(tokens(&json!({"access_token":"a","refresh_token":"r","expires_in":60,"resource_url":"portal.qwen.ai"}),None).is_ok());assert!(tokens(&json!({"access_token":"a","refresh_token":"r","expires_in":60,"resource_url":"portal.qwen.ai.evil.test"}),None).is_err());}
 #[tokio::test]async fn device_poll_sends_verifier_and_handles_pending(){
  use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;use std::convert::Infallible;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,mut rx)=tokio::sync::mpsc::channel(2);
  tauri::async_runtime::spawn(async move{for i in 0..2{let(socket,_)=listener.accept().await.unwrap();let tx=tx.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{tx.send(request.into_body().collect().await.unwrap().to_bytes()).await.unwrap();let body=if i==0{br#"{"error":"authorization_pending"}"#.as_slice()}else{br#"{"access_token":"access","refresh_token":"refresh","token_type":"Bearer","expires_in":3600,"resource_url":"portal.qwen.ai"}"#.as_slice()};Ok::<_,Infallible>(Response::new(Full::new(Bytes::copy_from_slice(body))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
  let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let(_tx,cancel)=tokio::sync::watch::channel(false);let mut pending=Pending{code:Zeroizing::new("dev+code".into()),verifier:Zeroizing::new("verifier".into()),deadline:Instant::now()+Duration::from_secs(5),interval:0,cancel};let(access,refresh,_)=poll(&client,format!("http://{addr}/token").parse().unwrap(),&mut pending).await.unwrap();assert_eq!(access.as_str(),"access");assert_eq!(refresh.as_str(),"refresh");for _ in 0..2{let body=rx.recv().await.unwrap();let body=std::str::from_utf8(&body).unwrap();assert!(body.contains("device_code=dev%2Bcode"));assert!(body.contains("code_verifier=verifier"));}
 }
}
