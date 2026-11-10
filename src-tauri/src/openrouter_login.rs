//! OpenRouter's browser authorization flow. The resulting key is stored in the Windows vault.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
use bytes::Bytes;
use http_body_util::Full;
use hyper::{body::Incoming,service::service_fn,Request,Response,StatusCode};
use hyper_util::rt::TokioIo;
use sha2::{Digest,Sha256};
use std::{convert::Infallible,sync::{Arc,atomic::{AtomicBool,Ordering}},time::Duration};
use tokio::net::TcpListener;
use zeroize::Zeroizing;

static ACTIVE:AtomicBool=AtomicBool::new(false);
struct Guard;
impl Drop for Guard{fn drop(&mut self){ACTIVE.store(false,Ordering::Release)}}

pub(crate) fn challenge(verifier:&str)->String{URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))}
fn callback_code(uri:&hyper::Uri,expected_path:&str,expected_state:&str)->Option<String>{
    if uri.path()!=expected_path{return None}
    let query=uri.query()?;if query.len()>4096{return None}
    let fields:Vec<_>=reqwest::Url::parse(&format!("http://127.0.0.1/?{query}")).ok()?.query_pairs().map(|(a,b)|(a.into_owned(),b.into_owned())).collect();
    let state:Vec<_>=fields.iter().filter(|(key,_)|key=="state").collect();let code:Vec<_>=fields.iter().filter(|(key,_)|key=="code").collect();
    if state.len()!=1||code.len()!=1||state[0].1!=expected_state||code[0].1.is_empty()||code[0].1.len()>2048||code[0].1.chars().any(char::is_control){return None}
    Some(code[0].1.clone())
}

pub(crate) async fn callback(request:Request<Incoming>,path:Arc<String>,state:Arc<String>,sender:Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>)->Result<Response<Full<Bytes>>,Infallible>{
    let code=if request.method()==hyper::Method::GET{callback_code(request.uri(),&path,&state)}else{None};
    let accepted=code.is_some();
    if let Some(code)=code {if let Ok(mut sender)=sender.lock(){if let Some(sender)=sender.take(){let _=sender.send(code);}}}
    let body=if accepted{"Connexion reçue. Tu peux revenir dans Nivra."}else{"Cette demande de connexion est invalide."};
    Ok(Response::builder().status(if accepted{StatusCode::OK}else{StatusCode::BAD_REQUEST}).header("content-type","text/plain; charset=utf-8").header("cache-control","no-store").header("referrer-policy","no-referrer").body(Full::new(Bytes::from(body))).unwrap())
}

async fn exchange_code(client:&reqwest::Client,url:reqwest::Url,code:&str,verifier:&str)->Result<Zeroizing<String>,String>{
    let body=Zeroizing::new(serde_json::json!({"code":code,"code_verifier":verifier,"code_challenge_method":"S256"}).to_string());
    let mut response=client.post(url).header("content-type","application/json").body(body.to_string()).send().await.map_err(|_|"Échange OpenRouter impossible.")?;
    if !response.status().is_success(){return Err(format!("OpenRouter a refusé la connexion (HTTP {}).",response.status().as_u16()))}
    let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse OpenRouter interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse OpenRouter trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
    let mut data:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|"Réponse OpenRouter illisible.")?;
    Ok(Zeroizing::new(match data.get_mut("key"){Some(serde_json::Value::String(value)) if !value.is_empty()&&value.len()<=2000=>std::mem::take(value),_=>return Err("Clé OpenRouter absente.".into())}))
}

#[tauri::command]
pub async fn openrouter_login()->Result<String,String>{
    if ACTIVE.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("Une connexion OpenRouter est déjà en cours.".into())}let _guard=Guard;
    #[cfg(not(windows))] {return Err("Connexion OpenRouter disponible uniquement sur Windows.".into())}
    #[cfg(windows)] {
        let listener=TcpListener::bind("127.0.0.1:0").await.map_err(|_|"Impossible d’ouvrir le retour local.")?;
        let port=listener.local_addr().map_err(|_|"Retour local indisponible.")?.port();
        let verifier=Zeroizing::new(format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()));
        let state=Arc::new(format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple()));
        let path=Arc::new(format!("/callback/{}",uuid::Uuid::new_v4().simple()));
        let callback_url=format!("http://127.0.0.1:{port}{path}");
        let mut auth=reqwest::Url::parse("https://openrouter.ai/auth").unwrap();
        auth.query_pairs_mut().append_pair("callback_url",&callback_url).append_pair("state",&state).append_pair("code_challenge",&challenge(&verifier)).append_pair("code_challenge_method","S256").append_pair("key_label","Nivra");
        crate::browser::open(auth.as_str())?;
        let (tx,mut rx)=tokio::sync::oneshot::channel();let sender=Arc::new(std::sync::Mutex::new(Some(tx)));
        let code=tokio::time::timeout(Duration::from_secs(180),async {
            let mut count=0usize;
            loop{tokio::select!{
                result=&mut rx=>break result.map_err(|_|"Connexion interrompue.".to_string()),
                incoming=listener.accept()=>{let(socket,_)=incoming.map_err(|_|"Retour local interrompu.".to_string())?;count+=1;if count>32{return Err("Trop de demandes de connexion.".into())}let path=path.clone();let state=state.clone();let sender=sender.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move |request|callback(request,path.clone(),state.clone(),sender.clone()));let _=tokio::time::timeout(Duration::from_secs(5),hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service)).await;});}
            }}
        }).await.map_err(|_|"La connexion OpenRouter a expiré.".to_string())??;
        let code=Zeroizing::new(code);
        let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(15)).build().map_err(|_|"Connexion réseau indisponible.")?;
        let key=exchange_code(&client,reqwest::Url::parse("https://openrouter.ai/api/v1/auth/keys").unwrap(),&code,&verifier).await?;
        crate::vault::gateway_remember_key("https://openrouter.ai/api/v1".into(),key.to_string(),true,false).map_err(|_|"La clé a été créée chez OpenRouter mais son enregistrement dans le coffre Windows a échoué. Vérifie ou révoque cette clé sur OpenRouter avant de réessayer.".into())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use http_body_util::BodyExt;
    #[test] fn pkce_matches_rfc_example(){assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),"E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")}
    #[test] fn callback_requires_path_state_and_single_code(){
        let good:hyper::Uri="/callback/abc?code=hello&state=right".parse().unwrap();assert_eq!(callback_code(&good,"/callback/abc","right").as_deref(),Some("hello"));
        for uri in ["/callback/abc?code=hello&state=wrong","/wrong?code=hello&state=right","/callback/abc?code=a&code=b&state=right","/callback/abc?code=a&state=right&state=right"] {assert!(callback_code(&uri.parse().unwrap(),"/callback/abc","right").is_none())}
    }
    #[tokio::test] async fn callback_accepts_only_matching_browser_return(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(std::sync::Mutex::new(Some(tx)));
        tauri::async_runtime::spawn(async move{for _ in 0..2{let(socket,_)=listener.accept().await.unwrap();let sender=sender.clone();tauri::async_runtime::spawn(async move{let path=Arc::new("/callback/test".to_string());let state=Arc::new("right".to_string());let service=service_fn(move|request|callback(request,path.clone(),state.clone(),sender.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(client.get(format!("http://{address}/callback/test?code=bad&state=wrong")).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
        assert_eq!(client.get(format!("http://{address}/callback/test?code=good&state=right")).send().await.unwrap().status(),StatusCode::OK);
        assert_eq!(rx.await.unwrap(),"good");
    }
    #[tokio::test] async fn exchange_sends_code_and_verifier_to_simulated_provider(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{
            let path=request.uri().path().to_owned();let method=request.method().clone();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((path,method,body)).unwrap();
            Ok::<_,Infallible>(Response::builder().header("content-type","application/json").body(Full::new(Bytes::from_static(br#"{"key":"synthetic-key"}"#))).unwrap())
        }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let key=exchange_code(&client,format!("http://{address}/api/v1/auth/keys").parse().unwrap(),"test-code","test-verifier").await.unwrap();assert_eq!(key.as_str(),"synthetic-key");
        let(path,method,body)=rx.await.unwrap();assert_eq!(path,"/api/v1/auth/keys");assert_eq!(method,hyper::Method::POST);let json:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(json["code"],"test-code");assert_eq!(json["code_verifier"],"test-verifier");assert_eq!(json["code_challenge_method"],"S256");
    }
}
