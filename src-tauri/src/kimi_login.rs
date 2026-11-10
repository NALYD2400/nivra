//! Kimi Code device authorization. Credentials remain in the Windows vault.
use std::{sync::{Mutex,OnceLock},time::{Duration,Instant}};
use serde::Serialize;
use zeroize::Zeroizing;

const CLIENT_ID:&str="17e5f671-d194-4dfb-9706-5516cb48c098";
const BINDING:&str="https://auth.kimi.com/api/oauth/token";
const DEVICE_URL:&str="https://auth.kimi.com/api/oauth/device_authorization";
const TOKEN_URL:&str="https://auth.kimi.com/api/oauth/token";
const VERSION:&str="0.14.0";
struct Pending { code:Zeroizing<String>, device:String, deadline:Instant, interval:u64, cancel:tokio::sync::watch::Receiver<bool> }
static PENDING:OnceLock<Mutex<Option<Pending>>>=OnceLock::new();
static CANCEL:OnceLock<Mutex<Option<tokio::sync::watch::Sender<bool>>>>=OnceLock::new();
struct Cached { access:Zeroizing<String>, valid_until:Instant }
static REFRESH:OnceLock<tokio::sync::Mutex<std::collections::BTreeMap<String,Cached>>>=OnceLock::new();
#[derive(Serialize)] pub struct DevicePrompt { user_code:String, verification_uri:String }

fn client()->Result<reqwest::Client,String>{reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(15)).build().map_err(|_|"Connexion Kimi indisponible.".into())}
fn form(values:&[(&str,&str)])->Zeroizing<String>{let mut serializer=reqwest::Url::parse("https://example.invalid/").unwrap();{let mut query=serializer.query_pairs_mut();for (key,value) in values{query.append_pair(key,value);}}Zeroizing::new(serializer.query().unwrap_or("").to_owned())}
async fn request(client:&reqwest::Client,url:reqwest::Url,device:&str,body:&str)->Result<(u16,serde_json::Value),String>{
    tokio::time::timeout(Duration::from_secs(20),async{
        let mut response=client.post(url).header("user-agent",format!("KimiCLI/{VERSION}")).header("x-msh-platform","kimi_code_cli").header("x-msh-version",VERSION).header("x-msh-device-id",device).header("content-type","application/x-www-form-urlencoded").body(body.to_owned()).send().await.map_err(|_|"Impossible de joindre Kimi.")?;
        let status=response.status().as_u16();let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse Kimi interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse Kimi trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
        let value=serde_json::from_slice(&bytes).map_err(|_|"Réponse Kimi illisible.")?;Ok((status,value))
    }).await.map_err(|_|"Connexion Kimi expirée.".to_string())?
}
fn bounded(value:Option<&serde_json::Value>,max:usize)->Option<&str>{value?.as_str().filter(|s|!s.is_empty()&&s.len()<=max&&!s.chars().any(char::is_control))}
fn parse_device(value:&serde_json::Value,device:String)->Result<(Pending,DevicePrompt,tokio::sync::watch::Sender<bool>),String>{
    let code=bounded(value.get("device_code"),2048).ok_or("Code Kimi absent.")?;
    let user=bounded(value.get("user_code"),64).ok_or("Code utilisateur Kimi absent.")?;
    let uri=bounded(value.get("verification_uri_complete"),2048).or_else(||bounded(value.get("verification_uri"),2048)).ok_or("Adresse Kimi absente.")?;
    let url=reqwest::Url::parse(uri).map_err(|_|"Adresse Kimi invalide.")?;
    if url.scheme()!="https"||!matches!(url.host_str(),Some("auth.kimi.com"|"kimi.com"|"www.kimi.com"|"kimi.ai"|"www.kimi.ai"))||!url.username().is_empty()||url.password().is_some()||url.port().is_some() {return Err("Adresse de connexion Kimi inattendue.".into())}
    let seconds=value.get("expires_in").and_then(|v|v.as_u64()).unwrap_or(600).clamp(1,900);
    let interval=value.get("interval").and_then(|v|v.as_u64()).unwrap_or(5).clamp(1,30);
    let (sender,cancel)=tokio::sync::watch::channel(false);
    Ok((Pending{code:Zeroizing::new(code.to_owned()),device,deadline:Instant::now()+Duration::from_secs(seconds),interval,cancel},DevicePrompt{user_code:user.to_owned(),verification_uri:url.to_string()},sender))
}
fn tokens(value:&serde_json::Value,old:Option<&str>)->Result<(Zeroizing<String>,Zeroizing<String>,u64),String>{
    let access=bounded(value.get("access_token"),8192).ok_or("Jeton Kimi absent.")?;
    let refresh=match value.get("refresh_token"){
        Some(serde_json::Value::Null)|None=>old.ok_or("Renouvellement Kimi absent.")?,
        Some(value)=>bounded(Some(value),8192).ok_or("Renouvellement Kimi invalide.")?,
    };
    let expires=value.get("expires_in").and_then(|v|v.as_u64()).filter(|v|*v>0&&*v<=86400*30).ok_or("Durée de connexion Kimi invalide.")?;
    Ok((Zeroizing::new(access.to_owned()),Zeroizing::new(refresh.to_owned()),expires))
}
#[tauri::command] pub async fn kimi_login_start()->Result<DevicePrompt,String>{
    let device=uuid::Uuid::new_v4().simple().to_string();let client=client()?;
    let (status,value)=request(&client,reqwest::Url::parse(DEVICE_URL).unwrap(),&device,&form(&[("client_id",CLIENT_ID)])).await?;
    if !(200..300).contains(&status){return Err(format!("Connexion Kimi refusée (HTTP {status})."))}
    let (pending,prompt,sender)=parse_device(&value,device)?;
    {let mut slot=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kimi indisponible.")?;if let Some(previous)=slot.replace(sender){let _=previous.send(true);}}
    let mut slot=PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kimi indisponible.")?;
    *slot=Some(pending);Ok(prompt)
}
#[tauri::command] pub fn kimi_login_cancel()->Result<(),String>{if let Some(sender)=CANCEL.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kimi indisponible.")?.take(){let _=sender.send(true);}*PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kimi indisponible.")?=None;Ok(())}
async fn poll(client:&reqwest::Client,url:reqwest::Url,pending:&mut Pending)->Result<Zeroizing<String>,String>{
    let mut interval=pending.interval;
    loop{
        if *pending.cancel.borrow(){return Err("Connexion Kimi annulée.".into())}
        if Instant::now()>=pending.deadline{return Err("Le code Kimi a expiré.".into())}
        let payload=form(&[("client_id",CLIENT_ID),("device_code",&pending.code),("grant_type","urn:ietf:params:oauth:grant-type:device_code")]);
        let (status,value)=tokio::select!{result=request(client,url.clone(),&pending.device,&payload)=>result?,_ = pending.cancel.changed()=>return Err("Connexion Kimi annulée.".into())};
        if *pending.cancel.borrow(){return Err("Connexion Kimi annulée.".into())}
        if (200..300).contains(&status){let (_,refresh,_)=tokens(&value,None)?;return Ok(refresh)}
        match value.get("error").and_then(|v|v.as_str()){Some("authorization_pending")=>{},Some("slow_down")=>interval=(interval+5).min(60),Some("access_denied")=>return Err("Connexion Kimi refusée.".into()),Some("expired_token")=>return Err("Le code Kimi a expiré.".into()),_=>return Err(format!("Connexion Kimi impossible (HTTP {status})."))}
        let remaining=pending.deadline.saturating_duration_since(Instant::now());tokio::select!{_ = tokio::time::sleep(Duration::from_secs(interval).min(remaining))=>{},_ = pending.cancel.changed()=>return Err("Connexion Kimi annulée.".into())};
    }
}
#[tauri::command] pub async fn kimi_login_finish()->Result<String,String>{
    let mut pending=PENDING.get_or_init(||Mutex::new(None)).lock().map_err(|_|"Connexion Kimi indisponible.")?.take().ok_or("Aucune connexion Kimi en attente.")?;
    let refresh=poll(&client()?,reqwest::Url::parse(TOKEN_URL).unwrap(),&mut pending).await?;
    if *pending.cancel.borrow(){return Err("Connexion Kimi annulée.".into())};
    let stored=Zeroizing::new(serde_json::json!({"device":pending.device,"refresh":refresh.as_str()}).to_string());
    crate::vault::gateway_remember_key(BINDING.into(),stored.to_string(),false,false)
}

pub(crate) async fn access(reference:&str)->Result<Zeroizing<String>,String>{
    access_with(&client()?,reqwest::Url::parse(TOKEN_URL).unwrap(),reference).await
}
async fn access_with(client:&reqwest::Client,url:reqwest::Url,reference:&str)->Result<Zeroizing<String>,String>{
    let mut cache=REFRESH.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;
    let stored=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);
    cache.retain(|_,entry|entry.valid_until>Instant::now());
    if let Some(entry)=cache.get(reference){return Ok(Zeroizing::new(entry.access.to_string()))}
    let value:serde_json::Value=serde_json::from_str(&stored).map_err(|_|"Connexion Kimi enregistrée invalide.")?;
    let device=bounded(value.get("device"),64).ok_or("Appareil Kimi invalide.")?;
    if device.len()!=32||!device.bytes().all(|b|b.is_ascii_hexdigit()){return Err("Appareil Kimi invalide.".into())}
    let old=bounded(value.get("refresh"),8192).ok_or("Connexion Kimi enregistrée invalide.")?;
    let payload=form(&[("client_id",CLIENT_ID),("refresh_token",old),("grant_type","refresh_token")]);
    let (status,value)=request(client,url,device,&payload).await?;
    if !(200..300).contains(&status){return Err(format!("Renouvellement Kimi refusé (HTTP {status})."))}
    let (access,refresh,expires)=tokens(&value,Some(old))?;
    if refresh.as_str()!=old{let updated=Zeroizing::new(serde_json::json!({"device":device,"refresh":refresh.as_str()}).to_string());crate::vault::replace_key(reference,BINDING,false,false,&updated)?}
    if expires>60 {
        if cache.len()>=32{if let Some(oldest)=cache.iter().min_by_key(|(_,entry)|entry.valid_until).map(|(key,_)|key.clone()){cache.remove(&oldest);}}
        cache.insert(reference.to_owned(),Cached{access:Zeroizing::new(access.to_string()),valid_until:Instant::now()+Duration::from_secs(expires-60)});
    }
    Ok(access)
}
pub(crate) fn validate(reference:&str)->Result<(),String>{let _=Zeroizing::new(crate::vault::load_key(reference,BINDING,false,false)?);Ok(())}

#[derive(Serialize,Debug,PartialEq)]
pub struct UsageWindow{kind:String,used:Option<f64>,limit:Option<f64>,remaining:Option<f64>,used_ratio:Option<f64>,reset:Option<String>,duration_minutes:Option<u64>}
#[derive(Serialize,Debug,PartialEq)]
pub struct UsageWallet{total_cents:Option<f64>,remaining_cents:Option<f64>,currency:Option<String>,monthly_limit_cents:Option<f64>,monthly_used_cents:Option<f64>,monthly_limit_enabled:Option<bool>}
#[derive(Serialize,Debug,PartialEq)]
pub struct Usage{windows:Vec<UsageWindow>,wallet:Option<UsageWallet>}
fn usage_number(value:Option<&serde_json::Value>)->Option<f64>{let value=value?;let parsed=match value{serde_json::Value::Number(n)=>n.as_f64(),serde_json::Value::String(s) if !s.is_empty()&&s.len()<=40=>s.parse::<f64>().ok(),_=>None}?;if parsed.is_finite()&&parsed>=0.0{Some(parsed)}else{None}}
fn usage_date(value:Option<&serde_json::Value>)->Option<String>{value?.as_str().filter(|v|!v.is_empty()&&v.len()<=40&&v.bytes().all(|c|c.is_ascii_digit()||matches!(c,b'-'|b':'|b'T'|b'Z'|b'.'|b'+'))).map(str::to_owned)}
fn parse_usage(value:&serde_json::Value)->Result<Usage,String>{
 let root=value.as_object().ok_or("Réponse Kimi illisible.")?;
 let mut windows=Vec::new();
 let mut push_detail=|kind:&str,raw:&serde_json::Value,duration_minutes:Option<u64>|{
  let used=usage_number(raw.get("used"));let limit=usage_number(raw.get("limit"));let remaining=usage_number(raw.get("remaining"));let reset=usage_date(raw.get("resetTime").or_else(||raw.get("reset_time")));
  if used.is_some()||limit.is_some()||remaining.is_some()||reset.is_some(){windows.push(UsageWindow{kind:kind.into(),used,limit,remaining,used_ratio:None,reset,duration_minutes})}
 };
 if let Some(usage)=root.get("usage"){push_detail("principal",usage,None)}
 if let Some(total)=root.get("totalQuota"){push_detail("total",total,None)}
 if let Some(limits)=root.get("limits").and_then(|v|v.as_array()) {for row in limits.iter().take(20){let duration=row.get("window").and_then(|v|v.get("duration")).and_then(|v|v.as_u64()).filter(|v|*v>0&&*v<=525600);let unit=row.get("window").and_then(|v|v.get("timeUnit")).and_then(|v|v.as_str());let minutes=if unit==Some("TIME_UNIT_MINUTE"){duration}else{None};if let Some(detail)=row.get("detail"){push_detail("fenetre",detail,minutes)}}}
 let summaries=root.get("usages").or_else(||root.get("data").and_then(|v|v.get("quota")).and_then(|v|v.get("usages")));
 if let Some(summaries)=summaries.and_then(|v|v.as_object()){
  for(kind,keys) in [("5h",["limit_5h","limit5h"]),("7j",["limit_7d","limit7d"]),("mois_total",["limit_month_total","monthTotal"]),("mois_code",["limit_month_code","monthCode"])]{
   if let Some(row)=keys.iter().find_map(|key|summaries.get(*key)){let ratio=usage_number(row.get("used_ratio").or_else(||row.get("usedRatio")));let reset=usage_date(row.get("reset_time").or_else(||row.get("resetAt")));if ratio.is_some()||reset.is_some(){windows.push(UsageWindow{kind:kind.into(),used:None,limit:None,remaining:None,used_ratio:ratio,reset,duration_minutes:None})}}
  }
 }
 let wallet=root.get("boosterWallet").and_then(|v|v.as_object()).and_then(|wallet|{
  let balance=wallet.get("balance")?;if balance.get("type").and_then(|v|v.as_str())!=Some("BOOSTER"){return None}
  let total=usage_number(balance.get("amount")).map(|v|v/1_000_000.0);let remaining=usage_number(balance.get("amountLeft")).map(|v|v/1_000_000.0);
  let limit=wallet.get("monthlyChargeLimit");let used=wallet.get("monthlyUsed");let currency=limit.and_then(|v|v.get("currency")).or_else(||used.and_then(|v|v.get("currency"))).and_then(|v|v.as_str()).filter(|v|v.len()==3&&v.bytes().all(|b|b.is_ascii_uppercase())).map(str::to_owned);
  Some(UsageWallet{total_cents:total,remaining_cents:remaining,currency,monthly_limit_cents:usage_number(limit.and_then(|v|v.get("priceInCents"))),monthly_used_cents:usage_number(used.and_then(|v|v.get("priceInCents"))),monthly_limit_enabled:wallet.get("monthlyChargeLimitEnabled").and_then(|v|v.as_bool())})
 }).or_else(||root.get("data").and_then(|v|v.get("quota")).and_then(|v|v.get("extraUsage")).and_then(|wallet|wallet.as_object()).map(|wallet|UsageWallet{
  total_cents:usage_number(wallet.get("totalCents")),remaining_cents:usage_number(wallet.get("balanceCents")),currency:wallet.get("currency").and_then(|v|v.as_str()).filter(|v|v.len()==3&&v.bytes().all(|b|b.is_ascii_uppercase())).map(str::to_owned),monthly_limit_cents:usage_number(wallet.get("monthlyChargeLimitCents")),monthly_used_cents:usage_number(wallet.get("monthlyUsedCents")),monthly_limit_enabled:wallet.get("monthlyChargeLimitEnabled").and_then(|v|v.as_bool())
 }));
 Ok(Usage{windows,wallet})
}
async fn fetch_usage(client:&reqwest::Client,url:reqwest::Url,access:&str)->Result<Usage,String>{
 tokio::time::timeout(Duration::from_secs(20),async{
  let mut response=client.get(url).bearer_auth(access).header("accept","application/json").header("user-agent",format!("KimiCLI/{VERSION}")).header("x-msh-platform","kimi_code_cli").send().await.map_err(|_|"Impossible de lire les quotas Kimi.")?;
  if !response.status().is_success(){return Err(format!("Lecture des quotas Kimi refusée (HTTP {}).",response.status().as_u16()))}
  let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture Kimi interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse Kimi trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}let value=serde_json::from_slice(&bytes).map_err(|_|"Réponse Kimi illisible.")?;parse_usage(&value)
 }).await.map_err(|_|"La lecture des quotas Kimi a expiré.".to_string())?
}
#[tauri::command] pub async fn kimi_usage(reference:String)->Result<Usage,String>{let access=access(&reference).await?;fetch_usage(&client()?,"https://api.kimi.com/coding/v1/usages".parse().unwrap(),&access).await}

#[cfg(test)] mod tests{
 use super::*;use serde_json::json;
 #[test] fn kimi_usage_keeps_summary_and_detail_separate(){
  let data=json!({"usage":{"limit":"100","used":"3","remaining":"97","resetTime":"2026-12-01T00:00:00Z"},"limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},"detail":{"limit":"100","used":"100"}}],"usages":{"limit_5h":{"used_ratio":0,"reset_time":"2026-12-01T00:00:00Z"},"limit_month_total":{"used_ratio":"0.25"},"limit_month_code":{"used_ratio":-1}}});
  let result=parse_usage(&data).unwrap();assert_eq!(result.windows.len(),4);assert_eq!(result.windows[1].duration_minutes,Some(300));assert_eq!(result.windows[1].used,Some(100.0));assert_eq!(result.windows[2].used_ratio,Some(0.0));assert_eq!(result.windows[3].used_ratio,Some(0.25));
  let nested=parse_usage(&json!({"data":{"quota":{"usages":{"limit7d":{"usedRatio":0.5,"resetAt":"2026-12-01T00:00:00Z"}}}}})).unwrap();assert_eq!(nested.windows[0].kind,"7j");assert_eq!(nested.windows[0].used_ratio,Some(0.5));assert!(parse_usage(&json!(null)).is_err());assert!(parse_usage(&json!({})).unwrap().windows.is_empty());
  let wallet=parse_usage(&json!({"boosterWallet":{"balance":{"type":"BOOSTER","amount":"500000000","amountLeft":"250000000"},"monthlyChargeLimit":{"priceInCents":1000,"currency":"USD"},"monthlyChargeLimitEnabled":true}})).unwrap().wallet.unwrap();assert_eq!(wallet.total_cents,Some(500.0));assert_eq!(wallet.remaining_cents,Some(250.0));assert_eq!(wallet.currency.as_deref(),Some("USD"));assert_eq!(wallet.monthly_used_cents,None);
  let wallet=parse_usage(&json!({"data":{"quota":{"extraUsage":{"balanceCents":250,"totalCents":500,"currency":"USD"}}}})).unwrap().wallet.unwrap();assert_eq!(wallet.remaining_cents,Some(250.0));assert_eq!(wallet.total_cents,Some(500.0));
 }
 #[tokio::test] async fn kimi_usage_fetch_sends_bearer_and_parses_response(){
  use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_string();let path=request.uri().path().to_string();sender.lock().unwrap().take().unwrap().send((auth,path)).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"usage":{"limit":"100","remaining":"50"}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let result=fetch_usage(&client,format!("http://{address}/usages").parse().unwrap(),"synthetic").await.unwrap();assert_eq!(result.windows[0].remaining,Some(50.0));let(auth,path)=rx.await.unwrap();assert_eq!(auth,"Bearer synthetic");assert_eq!(path,"/usages");
 }
 #[test] fn device_prompt_accepts_only_kimi_https_hosts(){
  let base=json!({"device_code":"private","user_code":"ABCD","verification_uri":"https://www.kimi.com/oauth/device","expires_in":100,"interval":2});
  let (_,prompt,_sender)=parse_device(&base,"a".repeat(32)).unwrap();assert_eq!(prompt.user_code,"ABCD");assert_eq!(prompt.verification_uri,"https://www.kimi.com/oauth/device");
  let complete=json!({"device_code":"private","user_code":"ABCD","verification_uri":"","verification_uri_complete":"https://www.kimi.com/oauth/device?user_code=ABCD"});let (_,prompt,_sender)=parse_device(&complete,"a".repeat(32)).unwrap();assert_eq!(prompt.verification_uri,"https://www.kimi.com/oauth/device?user_code=ABCD");
  for url in ["http://www.kimi.com/oauth/device","https://kimi.com.evil.example/","https://evil.example/","https://user@kimi.com/","https://kimi.com:8443/"]{let mut value=base.clone();value["verification_uri"]=json!(url);assert!(parse_device(&value,"a".repeat(32)).is_err());}
 }
 #[test] fn token_response_requires_real_access_and_refresh(){
  let value=json!({"access_token":"access","refresh_token":"refresh","expires_in":3600});let (access,refresh,expires)=tokens(&value,None).unwrap();assert_eq!(expires,3600);assert_eq!(access.as_str(),"access");assert_eq!(refresh.as_str(),"refresh");
  assert!(tokens(&json!({"access_token":"access","expires_in":3600}),None).is_err());
  assert_eq!(tokens(&json!({"access_token":"access","expires_in":3600}),Some("previous")).unwrap().1.as_str(),"previous");
  assert!(tokens(&json!({"access_token":"access","refresh_token":"refresh"}),None).is_err());assert!(tokens(&json!({"access_token":"access","refresh_token":"refresh","expires_in":0}),None).is_err());assert!(tokens(&json!({"access_token":"access","refresh_token":"","expires_in":3600}),Some("previous")).is_err());
 }
 #[tokio::test] async fn token_exchange_posts_encoded_data_to_simulated_server(){
  use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let device=request.headers()["x-msh-device-id"].to_str().unwrap().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((device,body)).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"synthetic","refresh_token":"rotated","expires_in":3600}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let payload=form(&[("refresh_token","a+b&c"),("grant_type","refresh_token")]);let(status,value)=request(&client,format!("http://{address}/token").parse().unwrap(),"synthetic-device",&payload).await.unwrap();assert_eq!(status,200);assert_eq!(tokens(&value,None).unwrap().1.as_str(),"rotated");let(device,body)=rx.await.unwrap();assert_eq!(device,"synthetic-device");assert!(std::str::from_utf8(&body).unwrap().contains("refresh_token=a%2Bb%26c"));
 }
 #[tokio::test] async fn polling_waits_for_authorization_and_preserves_device(){
  use hyper::{body::Incoming,service::service_fn,Request,Response,StatusCode};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let (tx,rx)=tokio::sync::oneshot::channel();let received=std::sync::Arc::new(Mutex::new(Vec::new()));
  tauri::async_runtime::spawn(async move{for step in 0..2{let(socket,_)=listener.accept().await.unwrap();let seen=received.clone();let service=service_fn(move|request:Request<Incoming>|{let seen=seen.clone();async move{let device=request.headers()["x-msh-device-id"].to_str().unwrap().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();seen.lock().unwrap().push((device,body));let (status,body)=if step==0{(StatusCode::BAD_REQUEST,br#"{"error":"authorization_pending"}"#.as_slice())}else{(StatusCode::OK,br#"{"access_token":"temporary","refresh_token":"rotated","expires_in":3600}"#.as_slice())};Ok::<_,std::convert::Infallible>(Response::builder().status(status).body(Full::new(Bytes::copy_from_slice(body))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;}let _=tx.send(received.lock().unwrap().clone());});
  let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let (_sender,cancel)=tokio::sync::watch::channel(false);let mut pending=Pending{code:Zeroizing::new("device+code".into()),device:"synthetic".into(),deadline:Instant::now()+Duration::from_secs(3),interval:0,cancel};let refresh=poll(&client,format!("http://{address}/token").parse().unwrap(),&mut pending).await.unwrap();assert_eq!(refresh.as_str(),"rotated");let seen=rx.await.unwrap();assert_eq!(seen.len(),2);for (device,body) in seen{assert_eq!(device,"synthetic");let body=std::str::from_utf8(&body).unwrap();assert!(body.contains("device_code=device%2Bcode"));assert!(body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));}
 }

 #[cfg(windows)]
 #[tokio::test]
 #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
 async fn synthetic_vault_rotation_cache_and_removal(){
  use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;
  struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
  let original=json!({"device":"0123456789abcdef0123456789abcdef","refresh":"synthetic-old"}).to_string();
  let reference=crate::vault::gateway_remember_key(BINDING.into(),original,false,false).unwrap();let _cleanup=Cleanup(reference.clone());
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let (tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();if let Some(sender)=sender.lock().unwrap().take(){let _=sender.send(body);}Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap();let url: reqwest::Url=format!("http://{address}/token").parse().unwrap();
  let access=access_with(&client,url.clone(),&reference).await.unwrap();assert_eq!(access.as_str(),"synthetic-access");let sent=rx.await.unwrap();assert!(std::str::from_utf8(&sent).unwrap().contains("refresh_token=synthetic-old"));
  let record=Zeroizing::new(crate::vault::load_key(&reference,BINDING,false,false).unwrap());let parsed:serde_json::Value=serde_json::from_str(&record).unwrap();assert_eq!(parsed["refresh"],"synthetic-rotated");
  let cached=access_with(&client,url.clone(),&reference).await.unwrap();assert_eq!(cached.as_str(),"synthetic-access");
  crate::vault::gateway_forget_key(reference.clone()).unwrap();assert!(access_with(&client,url,&reference).await.is_err());
 }

 #[tokio::test] async fn cancellation_interrupts_pending_authorization(){
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let (connected_tx,connected_rx)=tokio::sync::oneshot::channel();
  let server=tauri::async_runtime::spawn(async move{let(_socket,_)=listener.accept().await.unwrap();let _=connected_tx.send(());std::future::pending::<()>().await;});
  let (sender,cancel)=tokio::sync::watch::channel(false);let mut pending=Pending{code:Zeroizing::new("code".into()),device:"synthetic".into(),deadline:Instant::now()+Duration::from_secs(30),interval:1,cancel};
  let task=tauri::async_runtime::spawn(async move{let client=reqwest::Client::builder().no_proxy().build().unwrap();poll(&client,format!("http://{address}/token").parse().unwrap(),&mut pending).await});
  tokio::time::timeout(Duration::from_secs(2),connected_rx).await.unwrap().unwrap();sender.send(true).unwrap();let result=tokio::time::timeout(Duration::from_secs(2),task).await.unwrap().unwrap();assert!(result.unwrap_err().contains("annulée"));server.abort();
 }

}
