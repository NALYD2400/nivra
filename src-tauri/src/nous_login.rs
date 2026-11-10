//! Nous Portal device authorization and single-use refresh rotation.
use std::{sync::{Mutex,OnceLock},time::{Duration,Instant}};
use serde::Serialize;
use serde_json::{json,Value};
use zeroize::Zeroizing;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
const CLIENT:&str="hermes-cli";
const DEVICE:&str="https://portal.nousresearch.com/api/oauth/device/code";
const TOKEN:&str="https://portal.nousresearch.com/api/oauth/token";
const BINDING:&str="https://portal.nousresearch.com/api/oauth/token";
const DEFAULT_BASE:&str="https://inference-api.nousresearch.com/v1";
struct Pending{code:Zeroizing<String>,deadline:Instant,interval:u64,cancel:tokio::sync::watch::Receiver<bool>}
static PENDING:OnceLock<Mutex<Option<Pending>>>=OnceLock::new();
static CANCEL:OnceLock<Mutex<Option<tokio::sync::watch::Sender<bool>>>>=OnceLock::new();
struct Cached{token:Zeroizing<String>,base:reqwest::Url,until:Instant}
static CACHE:OnceLock<tokio::sync::Mutex<std::collections::BTreeMap<String,Cached>>>=OnceLock::new();
#[derive(Serialize)]pub struct Prompt{user_code:String,verification_uri:String}
fn bounded(value:Option<&Value>,max:usize)->Option<&str>{value?.as_str().filter(|s|!s.is_empty()&&s.len()<=max&&!s.chars().any(char::is_control))}
fn base_url(raw:Option<&str>)->Result<reqwest::Url,String>{
 let url=reqwest::Url::parse(raw.unwrap_or(DEFAULT_BASE)).map_err(|_|"Adresse Nous invalide.")?;
 if url.scheme()!="https"||!matches!(url.host_str(),Some("inference-api.nousresearch.com"|"welcome-api.nousresearch.com"))||!url.username().is_empty()||url.password().is_some()||url.port().is_some()||!matches!(url.path(),"/v1"|"/v1/")||url.query().is_some()||url.fragment().is_some(){return Err("Adresse Nous inattendue.".into())}
 reqwest::Url::parse(&format!("https://{}/v1/",url.host_str().unwrap())).map_err(|_|"Adresse Nous invalide.".into())
}
fn client()->Result<reqwest::Client,String>{reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build().map_err(|_|"Connexion Nous indisponible.".into())}
fn form(values:&[(&str,&str)])->Zeroizing<String>{let mut url=reqwest::Url::parse("https://example.invalid/").unwrap();{let mut pairs=url.query_pairs_mut();for(k,v)in values{pairs.append_pair(k,v);}}Zeroizing::new(url.query().unwrap_or("").to_owned())}
async fn post(client:&reqwest::Client,url:reqwest::Url,body:&str,refresh:Option<&str>)->Result<(u16,Value),String>{
 let mut request=client.post(url).header("accept","application/json").header("content-type","application/x-www-form-urlencoded");
 if let Some(refresh)=refresh{request=request.header("x-nous-refresh-token",refresh)}
 let mut response=request.body(body.to_owned()).send().await.map_err(|_|"Impossible de joindre Nous Portal.")?;
 let status=response.status().as_u16();let mut data=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse Nous interrompue.")?{if data.len()+chunk.len()>65536{return Err("Réponse Nous trop volumineuse.".into())}data.extend_from_slice(&chunk)}Ok((status,serde_json::from_slice(&data).map_err(|_|"Réponse Nous illisible.")?))
}
fn parse_device(value:&Value)->Result<(Pending,Prompt,tokio::sync::watch::Sender<bool>),String>{
 let code=bounded(value.get("device_code"),2048).ok_or("Code Nous absent.")?;let user=bounded(value.get("user_code"),64).ok_or("Code utilisateur Nous absent.")?;
 let uri=bounded(value.get("verification_uri_complete"),2048).or_else(||bounded(value.get("verification_uri"),2048)).or_else(||bounded(value.get("verification_url"),2048)).ok_or("Adresse Nous absente.")?;
 let url=reqwest::Url::parse(uri).map_err(|_|"Adresse Nous invalide.")?;if url.scheme()!="https"||url.host_str()!=Some("portal.nousresearch.com")||!url.username().is_empty()||url.password().is_some()||url.port().is_some(){return Err("Adresse de connexion Nous inattendue.".into())}
 let seconds=value.get("expires_in").and_then(Value::as_u64).unwrap_or(600).clamp(1,900);let interval=value.get("interval").and_then(Value::as_u64).unwrap_or(5).max(1);if interval>120{return Err("Délai Nous trop long.".into())}
 let(sender,cancel)=tokio::sync::watch::channel(false);Ok((Pending{code:Zeroizing::new(code.into()),deadline:Instant::now()+Duration::from_secs(seconds),interval,cancel},Prompt{user_code:user.into(),verification_uri:url.to_string()},sender))
}
fn access_details(value:&Value,fallback:Option<&str>)->Result<(Zeroizing<String>,reqwest::Url,u64),String>{
 let token=bounded(value.get("access_token"),16384).ok_or("Jeton Nous absent.")?;
 let expiry=value.get("expires_in").and_then(Value::as_u64).filter(|v|*v>0&&*v<=30*86400).ok_or("Durée Nous invalide.")?;
 if value.get("token_type").and_then(Value::as_str).is_some_and(|v|!v.eq_ignore_ascii_case("bearer")){return Err("Type de jeton Nous invalide.".into())}
 // The token arrives over TLS from Nous; decoding only checks that its declared scope is suitable.
 let claims=token.split('.').nth(1).and_then(|part|URL_SAFE_NO_PAD.decode(part).ok()).and_then(|bytes|serde_json::from_slice::<Value>(&bytes).ok());
 let scope=claims.as_ref().and_then(|v|v.get("scope")).and_then(Value::as_str).or_else(||value.get("scope").and_then(Value::as_str)).ok_or("Permission du jeton Nous absente.")?;
 if !scope.split_whitespace().any(|item|item=="inference:invoke"){return Err("Permission d’utilisation Nous absente.".into())}
 let base=base_url(value.get("inference_base_url").and_then(Value::as_str).or(fallback))?;
 Ok((Zeroizing::new(token.to_owned()),base,expiry))
}
#[tauri::command]pub async fn nous_login_start()->Result<Prompt,String>{
 let(status,value)=post(&client()?,DEVICE.parse().unwrap(),&form(&[("client_id",CLIENT),("scope","inference:invoke")]),None).await?;if !(200..300).contains(&status){return Err(format!("Connexion Nous refusée (HTTP {status})."))}
 let(pending,prompt,sender)=parse_device(&value)?;{let mut slot=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Nous indisponible.")?;if let Some(previous)=slot.replace(sender){let _=previous.send(true);}}*PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Nous indisponible.")?=Some(pending);Ok(prompt)
}
#[tauri::command]pub fn nous_login_cancel()->Result<(),String>{if let Some(sender)=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Nous indisponible.")?.take(){let _=sender.send(true);}*PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Nous indisponible.")?=None;Ok(())}
async fn poll(client:&reqwest::Client,url:reqwest::Url,pending:&mut Pending)->Result<Value,String>{let mut interval=pending.interval;loop{
 if *pending.cancel.borrow(){return Err("Connexion Nous annulée.".into())}if Instant::now()>=pending.deadline{return Err("Le code Nous a expiré.".into())}
 let payload=form(&[("grant_type","urn:ietf:params:oauth:grant-type:device_code"),("client_id",CLIENT),("device_code",&pending.code)]);
 let(status,value)=tokio::select!{result=post(client,url.clone(),&payload,None)=>result?,_=pending.cancel.changed()=>return Err("Connexion Nous annulée.".into())};
 if (200..300).contains(&status)&&value.get("access_token").and_then(Value::as_str).is_some(){return Ok(value)}
 match value.get("error").and_then(Value::as_str){Some("authorization_pending")=>{},Some("slow_down")=>{interval+=5;if interval>120{return Err("Délai Nous trop long.".into())}},Some("access_denied")=>return Err("Connexion Nous refusée.".into()),Some("expired_token")=>return Err("Le code Nous a expiré.".into()),_=>return Err(format!("Connexion Nous impossible (HTTP {status})."))}
 let remaining=pending.deadline.saturating_duration_since(Instant::now());tokio::select!{_=tokio::time::sleep(Duration::from_secs(interval).min(remaining))=>{},_=pending.cancel.changed()=>return Err("Connexion Nous annulée.".into())};
}}
#[tauri::command]pub async fn nous_login_finish()->Result<String,String>{
 let mut pending=PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Nous indisponible.")?.take().ok_or("Aucune connexion Nous en attente.")?;let value=poll(&client()?,TOKEN.parse().unwrap(),&mut pending).await?;if *pending.cancel.borrow(){return Err("Connexion Nous annulée.".into())}
 let refresh=bounded(value.get("refresh_token"),8192).ok_or("Renouvellement Nous absent.")?;let(token,base,expiry)=access_details(&value,None)?;let stored=Zeroizing::new(json!({"refresh":refresh,"base":base.as_str()}).to_string());let reference=crate::vault::gateway_remember_key(BINDING.into(),stored.to_string(),false,false)?;
 if expiry>120{let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;if cache.len()>=32{if let Some(k)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&k);}}cache.insert(reference.clone(),Cached{token,base,until:Instant::now()+Duration::from_secs(expiry-120)});}Ok(reference)
}
pub(crate) fn validate(reference:&str)->Result<(),String>{let _=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);Ok(())}
#[cfg(all(test,windows))]pub(crate) async fn synthetic_cached_session(base:reqwest::Url)->String{let stored=json!({"refresh":"synthetic-refresh","base":DEFAULT_BASE}).to_string();let reference=crate::vault::gateway_remember_key(BINDING.into(),stored,false,false).unwrap();CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await.insert(reference.clone(),Cached{token:Zeroizing::new("synthetic-nous-access".into()),base,until:Instant::now()+Duration::from_secs(60)});reference}
pub(crate) async fn access(reference:&str)->Result<(Zeroizing<String>,reqwest::Url),String>{access_with(&client()?,TOKEN.parse().unwrap(),reference).await}
async fn access_with(client:&reqwest::Client,url:reqwest::Url,reference:&str)->Result<(Zeroizing<String>,reqwest::Url),String>{
 let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;let stored=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);cache.retain(|_,v|v.until>Instant::now());if let Some(v)=cache.get(reference){return Ok((Zeroizing::new(v.token.to_string()),v.base.clone()))}
 let old:Value=serde_json::from_str(&stored).map_err(|_|"Connexion Nous enregistrée invalide.")?;let refresh=bounded(old.get("refresh"),8192).ok_or("Renouvellement Nous absent.")?;
 let(status,value)=post(client,url,&form(&[("grant_type","refresh_token"),("client_id",CLIENT)]),Some(refresh)).await?;if !(200..300).contains(&status){return Err(format!("Renouvellement Nous refusé (HTTP {status})."))}
 // Store the rotated credential before validating the access response: the old one may already be invalid.
 let rotated=bounded(value.get("refresh_token"),8192).unwrap_or(refresh);let prior_base=bounded(old.get("base"),256).unwrap_or(DEFAULT_BASE);let next=Zeroizing::new(json!({"refresh":rotated,"base":prior_base}).to_string());if rotated!=refresh{crate::vault::replace_key(reference,BINDING,false,false,&next)?}
 let(token,base,expiry)=access_details(&value,Some(prior_base))?;if base.as_str()!=prior_base{let next=Zeroizing::new(json!({"refresh":rotated,"base":base.as_str()}).to_string());crate::vault::replace_key(reference,BINDING,false,false,&next)?}
 if expiry>120{if cache.len()>=32{if let Some(k)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&k);}}cache.insert(reference.into(),Cached{token:Zeroizing::new(token.to_string()),base:base.clone(),until:Instant::now()+Duration::from_secs(expiry-120)});}Ok((token,base))
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn validates_login_and_inference_hosts(){let good=json!({"device_code":"secret","user_code":"ABCD","verification_uri":"https://portal.nousresearch.com/manage-subscription"});assert!(parse_device(&good).is_ok());let mut bad=good;bad["verification_uri"]=json!("https://portal.nousresearch.com.evil.test/");assert!(parse_device(&bad).is_err());for host in ["https://inference-api.nousresearch.com/v1","https://welcome-api.nousresearch.com/v1/"]{assert!(base_url(Some(host)).is_ok())}for host in ["https://inference-api.nousresearch.com.evil.test/v1","http://inference-api.nousresearch.com/v1","https://inference-api.nousresearch.com/v1?x=1"]{assert!(base_url(Some(host)).is_err())}}
 #[test]fn requires_inference_scope_and_preserves_base(){let good=json!({"access_token":"synthetic","expires_in":600,"scope":"inference:invoke"});assert_eq!(access_details(&good,Some("https://welcome-api.nousresearch.com/v1")).unwrap().1.as_str(),"https://welcome-api.nousresearch.com/v1/");let mut bad=good;bad["scope"]=json!("billing:manage");assert!(access_details(&bad,None).is_err());}
 #[tokio::test]async fn device_poll_handles_pending_and_cancellation(){
  use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;use std::convert::Infallible;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,mut rx)=tokio::sync::mpsc::channel(2);
  tauri::async_runtime::spawn(async move{for i in 0..2{let(socket,_)=listener.accept().await.unwrap();let tx=tx.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();tx.send(body).await.unwrap();let data=if i==0{br#"{"error":"authorization_pending"}"#.as_slice()}else{br#"{"access_token":"synthetic","refresh_token":"refresh","expires_in":600,"scope":"inference:invoke"}"#.as_slice()};Ok::<_,Infallible>(Response::new(Full::new(Bytes::copy_from_slice(data))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
  let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let(sender,cancel)=tokio::sync::watch::channel(false);let mut pending=Pending{code:Zeroizing::new("device+code".into()),deadline:Instant::now()+Duration::from_secs(5),interval:0,cancel};let result=poll(&client,format!("http://{address}/token").parse().unwrap(),&mut pending).await.unwrap();assert_eq!(result["access_token"],"synthetic");for _ in 0..2{let body=rx.recv().await.unwrap();let text=std::str::from_utf8(&body).unwrap();assert!(text.contains("device_code=device%2Bcode"));assert!(text.contains("client_id=hermes-cli"));}
  sender.send(true).unwrap();assert!(poll(&client,format!("http://{address}/token").parse().unwrap(),&mut pending).await.unwrap_err().contains("annulée"));
 }
 #[cfg(windows)]#[tokio::test]#[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]async fn preserves_rotated_refresh_when_access_is_invalid(){
  use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;use std::{convert::Infallible,sync::Arc};
  struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
  let stored=json!({"refresh":"old-refresh","base":DEFAULT_BASE}).to_string();let reference=crate::vault::gateway_remember_key(BINDING.into(),stored,false,false).unwrap();let _cleanup=Cleanup(reference.clone());
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let header=request.headers().get("x-nous-refresh-token").unwrap().to_str().unwrap().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((header,body)).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"bad","refresh_token":"rotated-refresh","expires_in":600,"scope":"billing:manage"}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();assert!(access_with(&client,format!("http://{address}/token").parse().unwrap(),&reference).await.is_err());let(header,body)=rx.await.unwrap();assert_eq!(header,"old-refresh");let body=std::str::from_utf8(&body).unwrap();assert!(body.contains("grant_type=refresh_token"));assert!(!body.contains("old-refresh"));let stored=Zeroizing::new(crate::vault::load_key(&reference,BINDING,false,false).unwrap());let result:Value=serde_json::from_str(&stored).unwrap();assert_eq!(result["refresh"],"rotated-refresh");
 }
}
