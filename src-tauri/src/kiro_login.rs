//! AWS Builder ID device flow for Kiro, with a per-account client registration.
use std::{sync::{Mutex,OnceLock},time::{Duration,Instant}};
use serde::Serialize;
use serde_json::{json,Value};
use zeroize::Zeroizing;
const ROOT:&str="https://oidc.us-east-1.amazonaws.com";
const BINDING:&str="https://oidc.us-east-1.amazonaws.com/token";
const GRANT:&str="urn:ietf:params:oauth:grant-type:device_code";
const START:&str="https://view.awsapps.com/start";
struct Pending{client_id:Zeroizing<String>,client_secret:Zeroizing<String>,client_expires_at:Option<u64>,device:Zeroizing<String>,deadline:Instant,interval:u64,cancel:tokio::sync::watch::Receiver<bool>}
static PENDING:OnceLock<Mutex<Option<Pending>>>=OnceLock::new();
static CANCEL:OnceLock<Mutex<Option<tokio::sync::watch::Sender<bool>>>>=OnceLock::new();
struct Cached{token:Zeroizing<String>,until:Instant}
static CACHE:OnceLock<tokio::sync::Mutex<std::collections::BTreeMap<String,Cached>>>=OnceLock::new();
#[derive(Serialize)]pub struct Prompt{user_code:String,verification_uri:String}
fn bounded(value:Option<&Value>,max:usize)->Option<&str>{value?.as_str().filter(|s|!s.is_empty()&&s.len()<=max&&!s.chars().any(char::is_control))}
fn client()->Result<reqwest::Client,String>{reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build().map_err(|_|"Connexion Kiro indisponible.".into())}
async fn post(client:&reqwest::Client,url:reqwest::Url,body:&Value)->Result<(u16,Value),String>{
 let data=Zeroizing::new(body.to_string());let mut response=client.post(url).header("accept","application/json").header("content-type","application/json").body(data.to_string()).send().await.map_err(|_|"Impossible de joindre AWS pour Kiro.")?;
 let status=response.status().as_u16();let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse AWS interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse AWS trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}Ok((status,serde_json::from_slice(&bytes).map_err(|_|"Réponse AWS illisible.")?))
}
fn parse_device(value:&Value,id:Zeroizing<String>,secret:Zeroizing<String>)->Result<(Pending,Prompt,tokio::sync::watch::Sender<bool>),String>{
 let code=bounded(value.get("deviceCode"),4096).ok_or("Code Kiro absent.")?;let user=bounded(value.get("userCode"),128).ok_or("Code utilisateur Kiro absent.")?;
 let uri=bounded(value.get("verificationUriComplete"),2048).or_else(||bounded(value.get("verificationUri"),2048)).ok_or("Adresse AWS absente.")?;let url=reqwest::Url::parse(uri).map_err(|_|"Adresse AWS invalide.")?;
 if url.scheme()!="https"||!matches!(url.host_str(),Some("device.sso.us-east-1.amazonaws.com"|"view.awsapps.com"))||!url.username().is_empty()||url.password().is_some()||url.port().is_some(){return Err("Adresse d’autorisation AWS inattendue.".into())}
 let seconds=value.get("expiresIn").and_then(Value::as_u64).unwrap_or(600).clamp(1,900);let interval=value.get("interval").and_then(Value::as_u64).unwrap_or(5).max(1);if interval>120{return Err("Délai AWS trop long.".into())}
 let(sender,cancel)=tokio::sync::watch::channel(false);Ok((Pending{client_id:id,client_secret:secret,client_expires_at:None,device:Zeroizing::new(code.into()),deadline:Instant::now()+Duration::from_secs(seconds),interval,cancel},Prompt{user_code:user.into(),verification_uri:url.to_string()},sender))
}
fn tokens(value:&Value,old:Option<&str>)->Result<(Zeroizing<String>,Zeroizing<String>,u64),String>{
 let token=bounded(value.get("accessToken"),16384).ok_or("Jeton Kiro absent.")?;let refresh=match value.get("refreshToken"){None|Some(Value::Null)=>old,Some(value)=>bounded(Some(value),8192)}.ok_or("Renouvellement Kiro absent.")?;
 let expires=value.get("expiresIn").and_then(Value::as_u64).filter(|v|*v>0&&*v<=30*86400).ok_or("Durée Kiro invalide.")?;if value.get("tokenType").and_then(Value::as_str).is_some_and(|s|!s.eq_ignore_ascii_case("bearer")){return Err("Type de jeton Kiro invalide.".into())}Ok((Zeroizing::new(token.into()),Zeroizing::new(refresh.into()),expires))
}
#[tauri::command]pub async fn kiro_builder_start()->Result<Prompt,String>{
 let client=client()?;let(status,registration)=post(&client,format!("{ROOT}/client/register").parse().unwrap(),&json!({"clientName":"Nivra","clientType":"public","scopes":["codewhisperer:completions","codewhisperer:analysis","codewhisperer:conversations","codewhisperer:taskassist"],"grantTypes":[GRANT,"refresh_token"]})).await?;if !(200..300).contains(&status){return Err(format!("Inscription AWS refusée (HTTP {status})."))}
 let id=Zeroizing::new(bounded(registration.get("clientId"),4096).ok_or("Client AWS absent.")?.to_owned());let secret=Zeroizing::new(bounded(registration.get("clientSecret"),4096).ok_or("Secret AWS absent.")?.to_owned());
 let(status,value)=post(&client,format!("{ROOT}/device_authorization").parse().unwrap(),&json!({"clientId":id.as_str(),"clientSecret":secret.as_str(),"startUrl":START})).await?;if !(200..300).contains(&status){return Err(format!("Autorisation AWS refusée (HTTP {status})."))}
 let(mut pending,prompt,sender)=parse_device(&value,id,secret)?;pending.client_expires_at=registration.get("clientSecretExpiresAt").and_then(Value::as_u64).filter(|v|*v>0);{let mut slot=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kiro indisponible.")?;if let Some(previous)=slot.replace(sender){let _=previous.send(true);}}*PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kiro indisponible.")?=Some(pending);Ok(prompt)
}
#[tauri::command]pub fn kiro_builder_cancel()->Result<(),String>{if let Some(sender)=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kiro indisponible.")?.take(){let _=sender.send(true);}*PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kiro indisponible.")?=None;Ok(())}
async fn poll(client:&reqwest::Client,url:reqwest::Url,pending:&mut Pending)->Result<(Zeroizing<String>,Zeroizing<String>,u64),String>{let mut interval=pending.interval;loop{
 if *pending.cancel.borrow(){return Err("Connexion Kiro annulée.".into())}if Instant::now()>=pending.deadline{return Err("Le code Kiro a expiré.".into())}
 let body=json!({"clientId":pending.client_id.as_str(),"clientSecret":pending.client_secret.as_str(),"deviceCode":pending.device.as_str(),"grantType":GRANT});let(status,value)=tokio::select!{result=post(client,url.clone(),&body)=>result?,_=pending.cancel.changed()=>return Err("Connexion Kiro annulée.".into())};
 if (200..300).contains(&status){return tokens(&value,None)}match value.get("error").and_then(Value::as_str){Some("authorization_pending")=>{},Some("slow_down")=>{interval+=5;if interval>120{return Err("Délai AWS trop long.".into())}},Some("access_denied")=>return Err("Connexion Kiro refusée.".into()),Some("expired_token")=>return Err("Le code Kiro a expiré.".into()),_=>return Err(format!("Connexion Kiro impossible (HTTP {status})."))}let remaining=pending.deadline.saturating_duration_since(Instant::now());tokio::select!{_=tokio::time::sleep(Duration::from_secs(interval).min(remaining))=>{},_=pending.cancel.changed()=>return Err("Connexion Kiro annulée.".into())};
}}
#[tauri::command]pub async fn kiro_builder_finish()->Result<String,String>{
 let mut pending=PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kiro indisponible.")?.take().ok_or("Aucune connexion Kiro en attente.")?;let(token,refresh,expires)=poll(&client()?,BINDING.parse().unwrap(),&mut pending).await?;if *pending.cancel.borrow(){return Err("Connexion Kiro annulée.".into())}
 let stored=Zeroizing::new(json!({"client_id":pending.client_id.as_str(),"client_secret":pending.client_secret.as_str(),"client_expires_at":pending.client_expires_at,"refresh":refresh.as_str()}).to_string());let reference=crate::vault::gateway_remember_key(BINDING.into(),stored.to_string(),false,false)?;if expires>120{let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;if cache.len()>=32{if let Some(k)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&k);}}cache.insert(reference.clone(),Cached{token,until:Instant::now()+Duration::from_secs(expires-120)});}Ok(reference)
}
pub(crate) fn validate(reference:&str)->Result<(),String>{let _=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);Ok(())}
#[tauri::command]pub async fn kiro_builder_models(credential_ref:String)->Result<Vec<Value>,String>{let token=access(&credential_ref).await?;crate::kiro_transport::catalog(token.as_str()).await}
pub(crate) async fn access(reference:&str)->Result<Zeroizing<String>,String>{
 #[cfg(test)]if reference=="synthetic-kiro-test"{return Ok(Zeroizing::new("synthetic-kiro-access".into()))}
 let mut cache=CACHE.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;let stored=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);cache.retain(|_,v|v.until>Instant::now());if let Some(v)=cache.get(reference){return Ok(Zeroizing::new(v.token.to_string()))}
 let old:Value=serde_json::from_str(&stored).map_err(|_|"Connexion Kiro enregistrée invalide.")?;let id=bounded(old.get("client_id"),4096).ok_or("Client Kiro absent.")?;let secret=bounded(old.get("client_secret"),4096).ok_or("Secret Kiro absent.")?;let refresh=bounded(old.get("refresh"),8192).ok_or("Renouvellement Kiro absent.")?;
 let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();if old.get("client_expires_at").and_then(Value::as_u64).is_some_and(|expiry|expiry>0&&expiry<=now){return Err("L’autorisation AWS Kiro a expiré. Reconnecte ce compte.".into())}
 let(status,value)=post(&client()?,BINDING.parse().unwrap(),&json!({"clientId":id,"clientSecret":secret,"refreshToken":refresh,"grantType":"refresh_token"})).await?;if !(200..300).contains(&status){return Err(if matches!(value.get("error").and_then(Value::as_str),Some("invalid_client"|"invalid_grant")){"La connexion AWS Kiro a expiré ou a été révoquée. Reconnecte ce compte.".into()}else{format!("Renouvellement Kiro refusé (HTTP {status}).")})}
 if let Some(rotated)=bounded(value.get("refreshToken"),8192).filter(|v|*v!=refresh){let next=Zeroizing::new(json!({"client_id":id,"client_secret":secret,"client_expires_at":old.get("client_expires_at"),"refresh":rotated}).to_string());crate::vault::replace_key(reference,BINDING,false,false,&next)?}
 let(token,_,expires)=tokens(&value,Some(refresh))?;if expires>120{if cache.len()>=32{if let Some(k)=cache.iter().min_by_key(|(_,v)|v.until).map(|(k,_)|k.clone()){cache.remove(&k);}}cache.insert(reference.into(),Cached{token:Zeroizing::new(token.to_string()),until:Instant::now()+Duration::from_secs(expires-120)});}Ok(token)
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn validates_aws_device_and_tokens(){let good=json!({"deviceCode":"device","userCode":"ABCD","verificationUri":"https://device.sso.us-east-1.amazonaws.com/","interval":5});let secret=||Zeroizing::new("synthetic".into());assert!(parse_device(&good,secret(),secret()).is_ok());let mut bad=good;bad["verificationUri"]=json!("https://device.sso.us-east-1.amazonaws.com.evil.test/");assert!(parse_device(&bad,secret(),secret()).is_err());assert!(tokens(&json!({"accessToken":"a","refreshToken":"r","expiresIn":3600}),None).is_ok());assert!(tokens(&json!({"accessToken":"a","refreshToken":"","expiresIn":3600}),Some("old")).is_err());}
 #[tokio::test]async fn polls_aws_device_flow_with_registered_client(){
  use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;use std::convert::Infallible;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,mut rx)=tokio::sync::mpsc::channel(2);
  tauri::async_runtime::spawn(async move{for i in 0..2{let(socket,_)=listener.accept().await.unwrap();let tx=tx.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();tx.send(body).await.unwrap();let(status,body)=if i==0{(400,br#"{"error":"authorization_pending"}"#.as_slice())}else{(200,br#"{"accessToken":"access","refreshToken":"refresh","expiresIn":3600}"#.as_slice())};Ok::<_,Infallible>(Response::builder().status(status).body(Full::new(Bytes::copy_from_slice(body))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
  let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let(sender,cancel)=tokio::sync::watch::channel(false);let mut pending=Pending{client_id:Zeroizing::new("client".into()),client_secret:Zeroizing::new("secret".into()),client_expires_at:None,device:Zeroizing::new("device".into()),deadline:Instant::now()+Duration::from_secs(5),interval:0,cancel};let(token,refresh,_)=poll(&client,format!("http://{address}/token").parse().unwrap(),&mut pending).await.unwrap();assert_eq!(token.as_str(),"access");assert_eq!(refresh.as_str(),"refresh");for _ in 0..2{let body=rx.recv().await.unwrap();let body:Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["clientId"],"client");assert_eq!(body["clientSecret"],"secret");assert_eq!(body["deviceCode"],"device");assert_eq!(body["grantType"],GRANT);}sender.send(true).unwrap();assert!(poll(&client,format!("http://{address}/token").parse().unwrap(),&mut pending).await.unwrap_err().contains("annulée"));
 }
}
