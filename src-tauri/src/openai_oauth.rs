//! Building blocks for Nivra's own OpenAI authorization; not exposed until the flow is complete.
use zeroize::Zeroizing;

const RESOURCE:&str="https://api.openai.com/v1";
const CALLBACK_PATH:&str="/auth/callback";
const SCOPES:&str="openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const JWKS:&str="https://auth.openai.com/.well-known/jwks.json";
const TOKEN_URL:&str="https://auth.openai.com/api/accounts/oauth/token";
const REVOKE_URL:&str="https://auth.openai.com/api/accounts/oauth/revoke";
const VAULT_BINDING:&str="https://auth.openai.com/";

struct Callback{code:Zeroizing<String>,client_id:String}
fn clean(value:&str,max:usize)->bool{!value.is_empty()&&value.len()<=max&&!value.chars().any(char::is_control)}
fn callback(uri:&hyper::Uri,expected_state:&str,previous_client:Option<&str>)->Result<Callback,String>{
 if uri.path()!=CALLBACK_PATH{return Err("Retour OpenAI invalide.".into())}
 let query=uri.query().filter(|v|v.len()<=8192).ok_or("Retour OpenAI invalide.")?;
 let parsed=reqwest::Url::parse(&format!("http://127.0.0.1/?{query}")).map_err(|_|"Retour OpenAI invalide.")?;
 let pairs:Vec<_>=parsed.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect();
 let one=|name:&str|->Result<Option<&str>,String>{let mut values=pairs.iter().filter(|(k,_)|k==name).map(|(_,v)|v.as_str());let first=values.next();if values.next().is_some(){return Err("Retour OpenAI invalide.".into())}Ok(first)};
 if one("state")?!=Some(expected_state){return Err("État de connexion OpenAI invalide.".into())}
 if one("error")?.is_some(){return Err("Autorisation OpenAI refusée ou annulée.".into())}
 let code=one("code")?.filter(|v|clean(v,4096)).ok_or("Code OpenAI absent.")?;
 let supplied=one("client_id")?;let client=match(previous_client,supplied){(Some(saved),Some(received)) if saved!=received=>return Err("Compte OpenAI inattendu.".into()),(Some(saved),_)=>saved,(None,Some(received))=>received,(None,None)=>return Err("Inscription OpenAI incomplète.".into())};
 if !client.starts_with("oaiapp_")||!clean(client,256){return Err("Identifiant OpenAI invalide.".into())}
 Ok(Callback{code:Zeroizing::new(code.to_owned()),client_id:client.to_owned()})
}
fn authorize_url(redirect:&str,host_id:&str,state:&str,nonce:&str,challenge:&str,client_id:Option<&str>)->Result<reqwest::Url,String>{
 let redirect=reqwest::Url::parse(redirect).map_err(|_|"Adresse de retour invalide.")?;
 if redirect.scheme()!="http"||redirect.host_str()!=Some("127.0.0.1")||redirect.port().is_none()||redirect.path()!=CALLBACK_PATH||redirect.query().is_some()||redirect.fragment().is_some()||!redirect.username().is_empty()||redirect.password().is_some(){return Err("Adresse de retour invalide.".into())}
 if ![host_id,state,nonce,challenge].iter().all(|v|clean(v,256)){return Err("Paramètres de connexion invalides.".into())}
 if client_id.is_some_and(|v|!v.starts_with("oaiapp_")||!clean(v,256)){return Err("Identifiant OpenAI invalide.".into())}
 let mut url=reqwest::Url::parse("https://auth.openai.com/api/accounts/authorize").unwrap();
 {let mut q=url.query_pairs_mut();q.append_pair("client_id",client_id.unwrap_or("dynamic_agent_client"));if client_id.is_none(){q.append_pair("agent_name_hint","Nivra");}q.append_pair("ext_agent_host_id",host_id).append_pair("response_type","code").append_pair("redirect_uri",redirect.as_str()).append_pair("scope",SCOPES).append_pair("resource",RESOURCE).append_pair("state",state).append_pair("nonce",nonce).append_pair("code_challenge_method","S256").append_pair("code_challenge",challenge);}
 Ok(url)
}
fn form(values:&[(&str,&str)])->Zeroizing<String>{let mut url=reqwest::Url::parse("https://example.invalid/").unwrap();{let mut q=url.query_pairs_mut();for(k,v)in values{q.append_pair(k,v);}}Zeroizing::new(url.query().unwrap_or("").to_owned())}
struct Tokens{access:Zeroizing<String>,refresh:Zeroizing<String>,id_token:Zeroizing<String>,expires:u64,scope:Zeroizing<String>}
fn parse_tokens(value:&serde_json::Value)->Result<Tokens,String>{
 let field=|name:&str,max:usize|value.get(name).and_then(|v|v.as_str()).filter(|v|clean(v,max)).ok_or_else(||"Réponse de connexion OpenAI incomplète.".to_owned());
 let access=field("access_token",16384)?;let refresh=field("refresh_token",16384)?;let id_token=field("id_token",32768)?;let scope=field("scope",2048)?;
 if !crate::openai_identity::plan_scope(scope)||!value.get("token_type").and_then(|v|v.as_str()).is_some_and(|v|v.eq_ignore_ascii_case("bearer")){return Err("L’accès à l’abonnement ChatGPT n’a pas été autorisé.".into())}
 let expires=value.get("expires_in").and_then(|v|v.as_u64()).filter(|v|*v>0&&*v<=86400*30).ok_or("Durée de connexion OpenAI invalide.")?;
 Ok(Tokens{access:Zeroizing::new(access.into()),refresh:Zeroizing::new(refresh.into()),id_token:Zeroizing::new(id_token.into()),expires,scope:Zeroizing::new(scope.into())})
}
async fn exchange(client:&reqwest::Client,url:reqwest::Url,callback:&Callback,redirect:&str,verifier:&str)->Result<Tokens,String>{
 let body=form(&[("grant_type","authorization_code"),("client_id",&callback.client_id),("code",&callback.code),("code_verifier",verifier),("redirect_uri",redirect),("resource",RESOURCE)]);
 let mut response=client.post(url).header("accept","application/json").header("content-type","application/x-www-form-urlencoded").body(body.to_string()).send().await.map_err(|_|"Connexion OpenAI impossible.")?;
 if !response.status().is_success(){return Err(format!("Connexion OpenAI refusée (HTTP {}).",response.status().as_u16()))}let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse OpenAI interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse OpenAI trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}let value=serde_json::from_slice(&bytes).map_err(|_|"Réponse OpenAI illisible.")?;parse_tokens(&value)
}

#[derive(serde::Serialize,serde::Deserialize)]
struct StoredSession{client_id:String,subject:String,refresh:String,scope:String}
impl Drop for StoredSession{fn drop(&mut self){use zeroize::Zeroize;self.refresh.zeroize();}}
impl StoredSession{
 fn validate(&self)->Result<(),String>{if !self.client_id.starts_with("oaiapp_")||!clean(&self.client_id,256)||!clean(&self.subject,512)||!clean(&self.refresh,16384)||!clean(&self.scope,2048){return Err("Connexion ChatGPT enregistrée invalide.".into())}Ok(())}
 fn encoded(&self)->Result<Zeroizing<String>,String>{self.validate()?;Ok(Zeroizing::new(serde_json::to_string(self).map_err(|_|"Enregistrement ChatGPT impossible.")?))}
}
fn store_session(client_id:&str,identity:&crate::openai_identity::VerifiedIdentity,tokens:&Tokens)->Result<String,String>{
 let session=StoredSession{client_id:client_id.into(),subject:identity.subject.clone(),refresh:tokens.refresh.to_string(),scope:tokens.scope.to_string()};let encoded=session.encoded()?;
 crate::vault::gateway_remember_key(VAULT_BINDING.into(),encoded.to_string(),false,false).map_err(|_|"Impossible d’enregistrer la connexion ChatGPT dans le coffre Windows. Réessaie la connexion.".into())
}
struct Refreshed{access:Zeroizing<String>,refresh:Zeroizing<String>,expires:u64,scope:Option<Zeroizing<String>>}
fn parse_refresh(value:&serde_json::Value)->Result<Refreshed,String>{
 let token=|name:&str|value.get(name).and_then(|v|v.as_str()).filter(|v|clean(v,16384)).ok_or("Renouvellement OpenAI incomplet.");
 let access=token("access_token")?;let refresh=token("refresh_token")?;
 if !value.get("token_type").and_then(|v|v.as_str()).is_some_and(|v|v.eq_ignore_ascii_case("bearer")){return Err("Jeton OpenAI invalide.".into())}
 let expires=value.get("expires_in").and_then(|v|v.as_u64()).filter(|v|*v>0&&*v<=86400*30).ok_or("Durée OpenAI invalide.")?;
 let scope=match value.get("scope"){None=>None,Some(v)=>{let v=v.as_str().filter(|s|clean(s,2048)).ok_or("Autorisation ChatGPT invalide.")?;Some(Zeroizing::new(v.to_owned()))}};
 Ok(Refreshed{access:Zeroizing::new(access.into()),refresh:Zeroizing::new(refresh.into()),expires,scope})
}
async fn refresh_request(client:&reqwest::Client,url:reqwest::Url,session:&StoredSession)->Result<Refreshed,String>{
 session.validate()?;let body=form(&[("grant_type","refresh_token"),("client_id",&session.client_id),("refresh_token",&session.refresh),("resource",RESOURCE)]);
 let mut response=client.post(url).header("accept","application/json").header("content-type","application/x-www-form-urlencoded").body(body.to_string()).send().await.map_err(|_|"Renouvellement OpenAI impossible.")?;
 if !response.status().is_success(){return Err(format!("Renouvellement OpenAI refusé (HTTP {}).",response.status().as_u16()))}let mut data=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Renouvellement OpenAI interrompu.")?{if data.len()+chunk.len()>65536{return Err("Réponse OpenAI trop volumineuse.".into())}data.extend_from_slice(&chunk)}let value=serde_json::from_slice(&data).map_err(|_|"Réponse OpenAI illisible.")?;parse_refresh(&value)
}
struct CachedAccess{access:Zeroizing<String>,until:std::time::Instant}
static REFRESH_LOCK:std::sync::OnceLock<tokio::sync::Mutex<std::collections::BTreeMap<String,CachedAccess>>>=std::sync::OnceLock::new();
#[cfg(all(test,windows))]
pub(crate) async fn synthetic_cached_session()->String{
 let session=StoredSession{client_id:"oaiapp_synthetic".into(),subject:"synthetic".into(),refresh:"synthetic-refresh".into(),scope:"chatgpt.tokens.use.direct".into()};let reference=crate::vault::gateway_remember_key(VAULT_BINDING.into(),session.encoded().unwrap().to_string(),false,false).unwrap();let mut cache=REFRESH_LOCK.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;cache.insert(reference.clone(),CachedAccess{access:Zeroizing::new("synthetic-openai-access".into()),until:std::time::Instant::now()+std::time::Duration::from_secs(60)});reference
}
pub(crate) fn validate(reference:&str)->Result<(),String>{let stored=Zeroizing::new(crate::vault::load_key(reference,VAULT_BINDING,false,false)?);let session:StoredSession=serde_json::from_str(&stored).map_err(|_|"Connexion ChatGPT enregistrée invalide.")?;session.validate()?;if !crate::openai_identity::plan_scope(&session.scope){return Err("Autorisation ChatGPT absente.".into())}Ok(())}
pub(crate) async fn access(reference:&str)->Result<Zeroizing<String>,String>{
 let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion OpenAI indisponible.")?;
 access_with(reference,&client,TOKEN_URL.parse().unwrap()).await
}

async fn revoke_request(client:&reqwest::Client,url:reqwest::Url,client_id:&str,refresh:&str)->bool{
 let body=form(&[("token",refresh),("token_type_hint","refresh_token"),("client_id",client_id)]);
 client.post(url).header("content-type","application/x-www-form-urlencoded").body(body.to_string()).send().await.is_ok_and(|response|response.status().as_u16()==200)
}
#[tauri::command] pub async fn chatgpt_plan_forget(reference:String)->Result<bool,String>{
 let mut cache=REFRESH_LOCK.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;
 let stored=Zeroizing::new(crate::vault::load_key(&reference,VAULT_BINDING,false,false)?);let session:StoredSession=serde_json::from_str(&stored).map_err(|_|"Connexion ChatGPT enregistrée invalide.")?;session.validate()?;
 let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion OpenAI indisponible.")?;
 let revoked=revoke_request(&client,REVOKE_URL.parse().unwrap(),&session.client_id,&session.refresh).await;
 cache.remove(&reference);crate::vault::gateway_forget_key(reference)?;Ok(revoked)
}
async fn access_with(reference:&str,client:&reqwest::Client,url:reqwest::Url)->Result<Zeroizing<String>,String>{
 let mut cache=REFRESH_LOCK.get_or_init(||tokio::sync::Mutex::new(std::collections::BTreeMap::new())).lock().await;
 let stored=Zeroizing::new(crate::vault::load_key(reference,VAULT_BINDING,false,false)?);let mut session:StoredSession=serde_json::from_str(&stored).map_err(|_|"Connexion ChatGPT enregistrée invalide.")?;session.validate()?;if !crate::openai_identity::plan_scope(&session.scope){return Err("L’autorisation ChatGPT est absente ; reconnecte ce compte.".into())}
 cache.retain(|_,entry|entry.until>std::time::Instant::now());if let Some(entry)=cache.get(reference){return Ok(Zeroizing::new(entry.access.to_string()))}
 let next=refresh_request(client,url,&session).await?;
 use zeroize::Zeroize;session.refresh.zeroize();session.refresh=next.refresh.to_string();if let Some(scope)=next.scope{session.scope=scope.to_string()}
 let updated=session.encoded()?;crate::vault::replace_key(reference,VAULT_BINDING,false,false,&updated).map_err(|_|"Le renouvellement a réussi mais n’a pas pu être enregistré. Reconnecte ce compte ChatGPT.".to_string())?;
 if !crate::openai_identity::plan_scope(&session.scope){return Err("L’autorisation ChatGPT est absente ; reconnecte ce compte.".into())}
 if next.expires>120{if cache.len()>=32{if let Some(key)=cache.iter().min_by_key(|(_,entry)|entry.until).map(|(key,_)|key.clone()){cache.remove(&key);}}cache.insert(reference.to_owned(),CachedAccess{access:Zeroizing::new(next.access.to_string()),until:std::time::Instant::now()+std::time::Duration::from_secs(next.expires-120)});}
 Ok(next.access)
}

async fn read_jwks(client:&reqwest::Client,url:reqwest::Url)->Result<jsonwebtoken::jwk::JwkSet,String>{
 let mut response=client.get(url).header("accept","application/json").send().await.map_err(|_|"Clés publiques OpenAI indisponibles.")?;
 if !response.status().is_success(){return Err(format!("Clés publiques OpenAI indisponibles (HTTP {}).",response.status().as_u16()))}
 let mut data=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture des clés OpenAI interrompue.")?{if data.len()+chunk.len()>131072{return Err("Clés publiques OpenAI trop volumineuses.".into())}data.extend_from_slice(&chunk)}
 let jwks:jsonwebtoken::jwk::JwkSet=serde_json::from_slice(&data).map_err(|_|"Clés publiques OpenAI illisibles.")?;
 if jwks.keys.is_empty()||jwks.keys.len()>64{return Err("Clés publiques OpenAI invalides.".into())}Ok(jwks)
}

async fn validated_identity(client:&reqwest::Client,tokens:&Tokens,client_id:&str,nonce:&str)->Result<crate::openai_identity::VerifiedIdentity,String>{
 let jwks=read_jwks(client,JWKS.parse().unwrap()).await?;
 crate::openai_identity::verify(&tokens.id_token,client_id,nonce,&jwks)
}

fn is_denied(uri:&hyper::Uri,state:&str)->bool{
 if uri.path()!=CALLBACK_PATH{return false}let Some(query)=uri.query().filter(|q|q.len()<=8192)else{return false};let Ok(url)=reqwest::Url::parse(&format!("http://127.0.0.1/?{query}"))else{return false};
 let pairs:Vec<_>=url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect();let states:Vec<_>=pairs.iter().filter(|(k,_)|k=="state").collect();let errors:Vec<_>=pairs.iter().filter(|(k,_)|k=="error").collect();states.len()==1&&states[0].1==state&&errors.len()==1&&errors[0].1=="access_denied"
}
async fn callback_response(request:hyper::Request<hyper::body::Incoming>,state:std::sync::Arc<String>,previous_client:Option<std::sync::Arc<String>>,sender:std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<Callback,String>>>>>)->Result<hyper::Response<http_body_util::Full<bytes::Bytes>>,std::convert::Infallible>{
 let result=if request.method()==hyper::Method::GET{callback(request.uri(),&state,previous_client.as_deref().map(String::as_str)).ok()}else{None};let accepted=result.is_some();
 let denied=request.method()==hyper::Method::GET&&is_denied(request.uri(),&state);let delivery=if let Some(result)=result{Some(Ok(result))}else if denied{Some(Err("Autorisation ChatGPT refusée.".into()))}else{None};
 if let Some(result)=delivery{if let Ok(mut sender)=sender.lock(){if let Some(sender)=sender.take(){let _=sender.send(result);}}}
 let body=if accepted{"Retour reçu. Tu peux revenir dans Nivra."}else if denied{"Autorisation refusée. Tu peux revenir dans Nivra."}else{"Cette demande de connexion est invalide."};
 Ok(hyper::Response::builder().status(if accepted||denied{hyper::StatusCode::OK}else{hyper::StatusCode::BAD_REQUEST}).header("content-type","text/plain; charset=utf-8").header("cache-control","no-store").header("referrer-policy","no-referrer").body(http_body_util::Full::new(bytes::Bytes::from(body))).unwrap())
}

fn host_id(directory:&std::path::Path)->Result<String,String>{
 use std::io::Write;
 std::fs::create_dir_all(directory).map_err(|_|"Dossier Nivra inaccessible.")?;let path=directory.join("openai-host-id");
 let candidate=format!("urn:uuid:{}",uuid::Uuid::new_v4());
 match std::fs::OpenOptions::new().write(true).create_new(true).open(&path){Ok(mut file)=>{file.write_all(candidate.as_bytes()).and_then(|_|file.sync_all()).map_err(|_|"Impossible de conserver l’identifiant de cet appareil.")?;Ok(candidate)},Err(error) if error.kind()==std::io::ErrorKind::AlreadyExists=>{let value=std::fs::read_to_string(path).map_err(|_|"Identifiant de cet appareil inaccessible.")?;let suffix=value.strip_prefix("urn:uuid:").ok_or("Identifiant de cet appareil invalide.")?;if uuid::Uuid::parse_str(suffix).is_err()||suffix.len()!=36{return Err("Identifiant de cet appareil invalide.".into())}Ok(value)},Err(_)=>Err("Impossible de conserver l’identifiant de cet appareil.".into())}
}
static LOGIN_ACTIVE:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
struct LoginGuard;
impl Drop for LoginGuard{fn drop(&mut self){LOGIN_ACTIVE.store(false,std::sync::atomic::Ordering::Release)}}
#[derive(serde::Serialize)] pub struct LoginResult{credential_ref:String,email:Option<String>}

#[tauri::command]
pub async fn chatgpt_plan_login(app:tauri::AppHandle)->Result<LoginResult,String>{
 use std::sync::atomic::Ordering;
 if LOGIN_ACTIVE.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("Une connexion ChatGPT est déjà en cours.".into())}let _guard=LoginGuard;
 #[cfg(not(windows))] {let _=app;Err("Connexion ChatGPT disponible uniquement sur Windows.".into())}
 #[cfg(windows)] {
  use std::sync::{Arc,Mutex};use tauri::Manager;use hyper::service::service_fn;use hyper_util::rt::TokioIo;
  let directory=app.path().app_data_dir().map_err(|_|"Dossier Nivra inaccessible.")?;let host=host_id(&directory)?;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|_|"Retour local indisponible.")?;let port=listener.local_addr().map_err(|_|"Retour local indisponible.")?.port();let redirect=format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
  let random=||format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple());let verifier=Zeroizing::new(random());let state=Arc::new(random());let nonce=Zeroizing::new(random());
  let auth=authorize_url(&redirect,&host,&state,&nonce,&crate::openrouter_login::challenge(&verifier),None)?;
  crate::browser::open(auth.as_str())?;
  let(tx,mut rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));
  let result=tokio::time::timeout(std::time::Duration::from_secs(180),async{let mut count=0;loop{tokio::select!{
   result=&mut rx=>break result.map_err(|_|"Connexion ChatGPT interrompue.".to_string())?,
   incoming=listener.accept()=>{let(socket,_)=incoming.map_err(|_|"Retour local interrompu.")?;count+=1;if count>32{return Err("Trop de demandes de connexion.".into())}let sender=sender.clone();let state=state.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|callback_response(request,state.clone(),None,sender.clone()));let _=tokio::time::timeout(std::time::Duration::from_secs(5),hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service)).await;});}
  }}}).await.map_err(|_|"La connexion ChatGPT a expiré.")??;
  let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion OpenAI indisponible.")?;
  let tokens=exchange(&client,TOKEN_URL.parse().unwrap(),&result,&redirect,&verifier).await?;
  let identity=match validated_identity(&client,&tokens,&result.client_id,&nonce).await{Ok(identity)=>identity,Err(message)=>{let revoked=revoke_request(&client,REVOKE_URL.parse().unwrap(),&result.client_id,&tokens.refresh).await;return Err(if revoked{message}else{format!("{message} La révocation n’a pas été confirmée ; vérifie les applications connectées dans ChatGPT.")})}};
  let reference=match store_session(&result.client_id,&identity,&tokens){Ok(reference)=>reference,Err(_)=>{let revoked=revoke_request(&client,REVOKE_URL.parse().unwrap(),&result.client_id,&tokens.refresh).await;return Err(if revoked{"La connexion n’a pas pu être enregistrée et son autorisation a été révoquée."}else{"La connexion n’a pas pu être enregistrée et sa révocation n’a pas été confirmée. Vérifie les applications connectées dans ChatGPT."}.into())}};Ok(LoginResult{credential_ref:reference,email:identity.email})
 }
}

#[cfg(test)] mod tests{
 use super::*;use serde_json::json;
 #[test] fn callback_requires_state_unique_code_and_bound_client(){let good:hyper::Uri="/auth/callback?state=abc&code=xyz&client_id=oaiapp_123".parse().unwrap();let c=callback(&good,"abc",None).unwrap();assert_eq!(c.client_id,"oaiapp_123");assert!(callback(&good,"wrong",None).is_err());assert!(callback(&good,"abc",Some("oaiapp_other")).is_err());for value in ["/auth/callback?state=abc&state=abc&code=x&client_id=oaiapp_1","/auth/callback?state=abc&code=x&client_id=dynamic_agent_client","/auth/callback?state=abc&error=access_denied&code=x&client_id=oaiapp_1"]{assert!(callback(&value.parse().unwrap(),"abc",None).is_err())}}
 #[test] fn host_identifier_is_stable_and_refuses_corrupt_file(){let temp=tempfile::tempdir().unwrap();let first=host_id(temp.path()).unwrap();assert_eq!(host_id(temp.path()).unwrap(),first);std::fs::write(temp.path().join("openai-host-id"),"broken").unwrap();assert!(host_id(temp.path()).is_err());}
 #[test] fn refusal_requires_matching_unique_state(){assert!(is_denied(&"/auth/callback?state=right&error=access_denied".parse().unwrap(),"right"));assert!(!is_denied(&"/auth/callback?state=wrong&error=access_denied".parse().unwrap(),"right"));assert!(!is_denied(&"/auth/callback?state=right&state=right&error=access_denied".parse().unwrap(),"right"));}
 #[test] fn request_uses_fixed_openai_origin_and_exact_loopback(){let url=authorize_url("http://127.0.0.1:1455/auth/callback","urn:uuid:host","state","nonce","challenge",None).unwrap();assert_eq!(url.host_str(),Some("auth.openai.com"));let pairs:std::collections::HashMap<_,_>=url.query_pairs().into_owned().collect();assert_eq!(pairs["client_id"],"dynamic_agent_client");assert_eq!(pairs["resource"],RESOURCE);assert_eq!(pairs["agent_name_hint"],"Nivra");assert!(authorize_url("http://localhost:1455/auth/callback","h","s","n","c",None).is_err());}
 #[test] fn tokens_require_plan_permission_and_refresh(){let good=json!({"access_token":"a","refresh_token":"r","id_token":"i","token_type":"Bearer","expires_in":3600,"scope":"openid chatgpt.tokens.use.direct"});let tokens=parse_tokens(&good).unwrap();assert_eq!(tokens.expires,3600);assert_eq!(tokens.access.as_str(),"a");assert_eq!(tokens.refresh.as_str(),"r");assert_eq!(tokens.id_token.as_str(),"i");assert!(tokens.scope.contains("chatgpt.tokens.use.direct"));let mut bad=good.clone();bad["scope"]=json!("openid");assert!(parse_tokens(&bad).is_err());bad=good;bad["refresh_token"]=json!("");assert!(parse_tokens(&bad).is_err());}
 #[test] fn refresh_requires_rotated_credentials_and_retains_granted_scope(){
  let data=json!({"access_token":"new-access","refresh_token":"new-refresh","token_type":"Bearer","expires_in":3600});let next=parse_refresh(&data).unwrap();assert_eq!(next.refresh.as_str(),"new-refresh");assert!(next.scope.is_none());assert!(parse_refresh(&json!({"access_token":"new-access","token_type":"Bearer","expires_in":3600})).is_err());
  let reduced=parse_refresh(&json!({"access_token":"a","refresh_token":"r","token_type":"Bearer","expires_in":3600,"scope":"openid"})).unwrap();assert!(!crate::openai_identity::plan_scope(reduced.scope.as_deref().unwrap()));
  let session=StoredSession{client_id:"oaiapp_test".into(),subject:"synthetic".into(),refresh:"refresh".into(),scope:"openid chatgpt.tokens.use.direct".into()};assert!(session.encoded().is_ok());
 }
 #[tokio::test] async fn refresh_posts_saved_client_and_resource_without_scope(){
  use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;use std::sync::{Arc,Mutex};
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send(body).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"new-access","refresh_token":"rotated","token_type":"Bearer","expires_in":3600}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let session=StoredSession{client_id:"oaiapp_test".into(),subject:"synthetic".into(),refresh:"old+refresh".into(),scope:"chatgpt.tokens.use.direct".into()};let next=refresh_request(&client,format!("http://{address}/token").parse().unwrap(),&session).await.unwrap();assert_eq!(next.refresh.as_str(),"rotated");let body=rx.await.unwrap();let body=std::str::from_utf8(&body).unwrap();assert!(body.contains("client_id=oaiapp_test"));assert!(body.contains("refresh_token=old%2Brefresh"));assert!(body.contains("resource=https%3A%2F%2Fapi.openai.com%2Fv1"));assert!(!body.contains("scope="));
 }
 #[tokio::test] async fn revoke_posts_refresh_to_selected_endpoint_and_checks_status(){
  use hyper::{body::Incoming,Request,Response,service::service_fn,StatusCode};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;use std::sync::{Arc,Mutex};
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send(body).unwrap();Ok::<_,std::convert::Infallible>(Response::builder().status(StatusCode::OK).body(Full::new(Bytes::new())).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(3)).build().unwrap();assert!(revoke_request(&client,format!("http://{address}/revoke").parse().unwrap(),"oaiapp_test","test+refresh").await);let body=rx.await.unwrap();let body=std::str::from_utf8(&body).unwrap();assert!(body.contains("token=test%2Brefresh"));assert!(body.contains("token_type_hint=refresh_token"));assert!(body.contains("client_id=oaiapp_test"));assert!(!revoke_request(&client,"http://127.0.0.1:1/revoke".parse().unwrap(),"oaiapp_test","refresh").await);
 }
 #[cfg(windows)]
 #[tokio::test]
 #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
 async fn synthetic_vault_refresh_rotates_and_stops_after_removal(){
  use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
  struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
  let session=StoredSession{client_id:"oaiapp_test".into(),subject:"synthetic".into(),refresh:"old-refresh".into(),scope:"chatgpt.tokens.use.direct".into()};let encoded=session.encoded().unwrap();let reference=crate::vault::gateway_remember_key(VAULT_BINDING.into(),encoded.to_string(),false,false).unwrap();let _cleanup=Cleanup(reference.clone());
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(|_:Request<Incoming>|async{Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"synthetic-access","refresh_token":"rotated-refresh","token_type":"Bearer","expires_in":3600}"#))))});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(3)).build().unwrap();let url=format!("http://{address}/token").parse().unwrap();assert_eq!(access_with(&reference,&client,url).await.unwrap().as_str(),"synthetic-access");let stored=Zeroizing::new(crate::vault::load_key(&reference,VAULT_BINDING,false,false).unwrap());let updated:StoredSession=serde_json::from_str(&stored).unwrap();assert_eq!(updated.refresh,"rotated-refresh");crate::vault::gateway_forget_key(reference.clone()).unwrap();assert!(access_with(&reference,&client,"http://127.0.0.1:1/".parse().unwrap()).await.is_err());
 }
 #[tokio::test] async fn exchange_sends_issued_client_and_exact_callback(){
  use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::{BodyExt,Full};use bytes::Bytes;use std::sync::{Arc,Mutex};
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send(body).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"a","refresh_token":"r","id_token":"i","token_type":"Bearer","expires_in":3600,"scope":"openid chatgpt.tokens.use.direct"}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let cb=Callback{code:Zeroizing::new("a+b".into()),client_id:"oaiapp_123".into()};let result=exchange(&client,format!("http://{address}/token").parse().unwrap(),&cb,"http://127.0.0.1:1455/auth/callback","verifier").await.unwrap();assert_eq!(result.access.as_str(),"a");let sent=rx.await.unwrap();let sent=std::str::from_utf8(&sent).unwrap();assert!(sent.contains("client_id=oaiapp_123"));assert!(sent.contains("code=a%2Bb"));assert!(sent.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback"));
 }
 #[tokio::test] async fn callback_listener_rejects_wrong_state_then_receives_client(){
  use std::sync::{Arc,Mutex};use hyper_util::rt::TokioIo;use hyper::service::service_fn;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));
  tauri::async_runtime::spawn(async move{for _ in 0..2{let(socket,_)=listener.accept().await.unwrap();let sender=sender.clone();tauri::async_runtime::spawn(async move{let state=Arc::new("expected".to_string());let service=service_fn(move|request|callback_response(request,state.clone(),None,sender.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();assert_eq!(client.get(format!("http://{address}/auth/callback?state=wrong&code=bad&client_id=oaiapp_test")).send().await.unwrap().status(),400);assert_eq!(client.get(format!("http://{address}/auth/callback?state=expected&code=good&client_id=oaiapp_test")).send().await.unwrap().status(),200);let cb=rx.await.unwrap().unwrap();assert_eq!(cb.code.as_str(),"good");assert_eq!(cb.client_id,"oaiapp_test");
 }
 #[tokio::test] async fn reads_only_bounded_jwk_set(){
  use std::sync::{Arc,Mutex};use hyper::{body::Incoming,Request,Response,service::service_fn};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{sender.lock().unwrap().take().unwrap().send(request.uri().path().to_string()).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"keys":[{"kty":"RSA","n":"AQAB","e":"AQAB","kid":"test"}]}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let jwks=read_jwks(&client,format!("http://{address}/jwks").parse().unwrap()).await.unwrap();assert_eq!(jwks.keys.len(),1);assert_eq!(rx.await.unwrap(),"/jwks");
 }
}
