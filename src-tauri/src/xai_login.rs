//! xAI browser OAuth. Refresh credentials stay bound to xAI in the Windows vault.
use std::{sync::{Arc,Mutex,OnceLock,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
use zeroize::Zeroizing;
use hyper_util::rt::TokioIo;
use hyper::service::service_fn;

const CLIENT_ID:&str="b1a00492-073a-47ea-816f-4c329264a828";
const DISCOVERY:&str="https://auth.x.ai/.well-known/openid-configuration";
const BINDING:&str="https://auth.x.ai/";
const REDIRECT:&str="http://127.0.0.1:56121/callback";
static ACTIVE:AtomicBool=AtomicBool::new(false);
struct Guard;
impl Drop for Guard{fn drop(&mut self){ACTIVE.store(false,Ordering::Release)}}
struct Cached{access:Zeroizing<String>,until:Instant}
static CACHE:OnceLock<tokio::sync::Mutex<std::collections::BTreeMap<String,Cached>>>=OnceLock::new();

fn client()->Result<reqwest::Client,String>{reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build().map_err(|_|"Connexion xAI indisponible.".into())}
fn endpoint(raw:&str)->Result<reqwest::Url,String>{
 let url=reqwest::Url::parse(raw).map_err(|_|"Adresse xAI invalide.")?;
 if url.scheme()!="https"||!matches!(url.host_str(),Some("auth.x.ai"|"accounts.x.ai"))||!url.username().is_empty()||url.password().is_some()||url.port().is_some(){return Err("Adresse de connexion xAI inattendue.".into())} Ok(url)
}
async fn json(mut response:reqwest::Response)->Result<serde_json::Value,String>{
 if !response.status().is_success(){return Err(format!("xAI a refusé la demande (HTTP {}).",response.status().as_u16()))}
 let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse xAI interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse xAI trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
 serde_json::from_slice(&bytes).map_err(|_|"Réponse xAI illisible.".into())
}
async fn discovery(client:&reqwest::Client)->Result<(reqwest::Url,reqwest::Url),String>{
 let value=json(client.get(DISCOVERY).header("accept","application/json").send().await.map_err(|_|"Impossible de joindre xAI.")?).await?;
 Ok((endpoint(value.get("authorization_endpoint").and_then(|v|v.as_str()).ok_or("Adresse xAI absente.")?)?,endpoint(value.get("token_endpoint").and_then(|v|v.as_str()).ok_or("Adresse xAI absente.")?)?))
}
fn token(value:&serde_json::Value,old:Option<&str>)->Result<(Zeroizing<String>,Zeroizing<String>,u64),String>{
 let bounded=|s:&str|!s.is_empty()&&s.len()<=8192&&!s.chars().any(char::is_control);
 let access=value.get("access_token").and_then(|v|v.as_str()).filter(|s|bounded(s)).ok_or("Jeton xAI absent.")?;
 let refresh=match value.get("refresh_token"){None|Some(serde_json::Value::Null)=>old,Some(v)=>v.as_str()}.filter(|s|bounded(s)).ok_or("Renouvellement xAI absent.")?;
 let expires=match value.get("expires_in"){None=>3600,Some(v)=>v.as_u64().filter(|v|*v>0&&*v<=86400*30).ok_or("Durée xAI invalide.")?};
 Ok((Zeroizing::new(access.to_owned()),Zeroizing::new(refresh.to_owned()),expires))
}
async fn exchange(client:&reqwest::Client,url:reqwest::Url,values:&[(&str,&str)],old:Option<&str>)->Result<(Zeroizing<String>,Zeroizing<String>,u64),String>{
 let mut body=reqwest::Url::parse("https://example.invalid/").unwrap();{let mut pairs=body.query_pairs_mut();for (key,value) in values{pairs.append_pair(key,value);}}
 let body=Zeroizing::new(body.query().unwrap_or("").to_owned());
 let value=json(client.post(url).header("accept","application/json").header("content-type","application/x-www-form-urlencoded").body(body.to_string()).send().await.map_err(|_|"Impossible de joindre xAI.")?).await?;
 token(&value,old)
}
#[tauri::command] pub async fn xai_login()->Result<String,String>{
 if ACTIVE.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("Une connexion xAI est déjà en cours.".into())}let _guard=Guard;
 #[cfg(not(windows))] {Err("Connexion xAI disponible uniquement sur Windows.".into())}
 #[cfg(windows)] {
  let listener=tokio::net::TcpListener::bind("127.0.0.1:56121").await.map_err(|_|"Le port de connexion xAI 56121 est déjà utilisé.")?;
  let client=client()?;let(mut auth,token_url)=discovery(&client).await?;
  let verifier=Zeroizing::new(format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()));
  let state=Arc::new(format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()));
  auth.query_pairs_mut().append_pair("response_type","code").append_pair("client_id",CLIENT_ID).append_pair("redirect_uri",REDIRECT).append_pair("scope","openid profile email offline_access grok-cli:access api:access").append_pair("code_challenge",&crate::openrouter_login::challenge(&verifier)).append_pair("code_challenge_method","S256").append_pair("state",&state).append_pair("nonce",&uuid::Uuid::new_v4().to_string());
  crate::browser::open(auth.as_str())?;
  let(tx,mut rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));let path=Arc::new("/callback".to_string());
  let code=tokio::time::timeout(Duration::from_secs(180),async{let mut count=0;loop{tokio::select!{
   result=&mut rx=>break result.map_err(|_|"Connexion xAI interrompue.".to_string()),
   incoming=listener.accept()=>{let(socket,_)=incoming.map_err(|_|"Retour local interrompu.")?;count+=1;if count>32{return Err("Trop de demandes de connexion.".into())}let path=path.clone();let state=state.clone();let sender=sender.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|crate::openrouter_login::callback(request,path.clone(),state.clone(),sender.clone()));let _=tokio::time::timeout(Duration::from_secs(5),hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service)).await;});}
  }}}).await.map_err(|_|"La connexion xAI a expiré.")??;
  let code=Zeroizing::new(code);let(_,refresh,_)=exchange(&client,token_url,&[("grant_type","authorization_code"),("client_id",CLIENT_ID),("code",&code),("redirect_uri",REDIRECT),("code_verifier",&verifier)],None).await?;
  crate::vault::gateway_remember_key(BINDING.into(),refresh.to_string(),false,false)
 }
}
pub(crate) fn validate(reference:&str)->Result<(),String>{let _=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);Ok(())}
#[cfg(all(test,windows))]
pub(crate) async fn synthetic_cached_session()->String{
 let reference=crate::vault::gateway_remember_key(BINDING.into(),"synthetic-refresh".into(),false,false).unwrap();
 CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await.insert(reference.clone(),Cached{access:Zeroizing::new("synthetic-grok-access".into()),until:Instant::now()+Duration::from_secs(60)});
 reference
}
pub(crate) async fn access(reference:&str)->Result<Zeroizing<String>,String>{
 let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;
 let old=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);cache.retain(|_,item|item.until>Instant::now());
 if let Some(item)=cache.get(reference){return Ok(Zeroizing::new(item.access.to_string()))}
 let client=client()?;let(_,url)=discovery(&client).await?;let(access,refresh,expires)=exchange(&client,url,&[("grant_type","refresh_token"),("client_id",CLIENT_ID),("refresh_token",&old)],Some(&old)).await?;
 if refresh.as_str()!=old.as_str(){crate::vault::replace_key(reference,BINDING,false,false,&refresh)?}
 if expires>120{if cache.len()>=32{if let Some(key)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&key);}}cache.insert(reference.into(),Cached{access:Zeroizing::new(access.to_string()),until:Instant::now()+Duration::from_secs(expires-120)});}
 Ok(access)
}
#[derive(serde::Serialize,Debug,PartialEq)]
pub struct GrokUsage{used_percent:Option<f64>,reset:Option<String>,period:Option<String>}
fn parse_usage(value:&serde_json::Value)->Result<GrokUsage,String>{
 let config=value.get("config").and_then(|v|v.as_object()).ok_or("Réponse des quotas Grok illisible.")?;
 let used_percent=config.get("creditUsagePercent").and_then(|v|v.as_f64()).filter(|n|n.is_finite()&&(0.0..=100.0).contains(n));
 let current=config.get("currentPeriod");
 let valid=|s:&&str|!s.is_empty()&&s.len()<=40&&s.bytes().all(|c|c.is_ascii_digit()||matches!(c,b'-'|b':'|b'T'|b'Z'|b'.'|b'+'));
 let reset=current.and_then(|v|v.get("end")).and_then(|v|v.as_str()).filter(valid).or_else(||config.get("billingPeriodEnd").and_then(|v|v.as_str()).filter(valid)).map(str::to_owned);
 let period=current.and_then(|v|v.get("type")).and_then(|v|v.as_str()).and_then(|s|match s{"USAGE_PERIOD_TYPE_WEEKLY"=>Some("hebdomadaire"),"USAGE_PERIOD_TYPE_MONTHLY"=>Some("mensuelle"),_=>None}).map(str::to_owned);
 Ok(GrokUsage{used_percent,reset,period})
}
#[tauri::command] pub async fn xai_usage(reference:String)->Result<GrokUsage,String>{
 let token=access(&reference).await?;
 let response=client()?.get("https://cli-chat-proxy.grok.com/v1/billing?format=credits").header("accept","application/json").header("x-xai-token-auth","xai-grok-cli").bearer_auth(token.as_str()).send().await.map_err(|_|"Impossible de lire les quotas Grok.")?;
 parse_usage(&json(response).await?)
}
#[cfg(test)] mod tests{
 use super::*;use serde_json::json;
 #[test] fn grok_quota_preserves_zero_and_unknown(){let row=parse_usage(&json!({"config":{"creditUsagePercent":0,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-11-15T12:00:00Z"}}})).unwrap();assert_eq!(row.used_percent,Some(0.0));assert_eq!(row.period.as_deref(),Some("hebdomadaire"));assert_eq!(row.reset.as_deref(),Some("2026-11-15T12:00:00Z"));assert_eq!(parse_usage(&json!({"config":{"creditUsagePercent":101}})).unwrap().used_percent,None);assert_eq!(parse_usage(&json!({"config":{}})).unwrap().used_percent,None);assert!(parse_usage(&json!({})).is_err());}
 #[test] fn endpoint_accepts_only_trusted_https_hosts(){assert!(endpoint("https://auth.x.ai/oauth/token").is_ok());assert!(endpoint("https://accounts.x.ai/login").is_ok());for value in ["http://auth.x.ai/token","https://auth.x.ai.evil.test/","https://u@auth.x.ai/","https://auth.x.ai:8443/"]{assert!(endpoint(value).is_err());}}
 #[test] fn tokens_require_valid_access_refresh_and_expiry(){assert!(token(&json!({"access_token":"a","refresh_token":"r","expires_in":3600}),None).is_ok());assert!(token(&json!({"access_token":"a"}),None).is_err());assert!(token(&json!({"access_token":"a","refresh_token":"","expires_in":10}),Some("old")).is_err());assert_eq!(token(&json!({"access_token":"a"}),Some("old")).unwrap().1.as_str(),"old");}
 #[tokio::test] async fn exchange_encodes_oauth_fields_and_parses_rotation(){
  use hyper::{body::Incoming,Request,Response};use http_body_util::{BodyExt,Full};use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send(body).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"synthetic","refresh_token":"rotated","expires_in":3600}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let(access,refresh,_)=exchange(&client,format!("http://{address}/token").parse().unwrap(),&[("refresh_token","a+b&c"),("grant_type","refresh_token")],None).await.unwrap();assert_eq!(access.as_str(),"synthetic");assert_eq!(refresh.as_str(),"rotated");let body=rx.await.unwrap();let body=std::str::from_utf8(&body).unwrap();assert!(body.contains("refresh_token=a%2Bb%26c"));assert!(body.contains("grant_type=refresh_token"));
 }
}
