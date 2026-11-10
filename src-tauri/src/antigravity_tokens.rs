//! Token exchange primitives for Antigravity. Not yet connected to the application.
use serde::Deserialize;
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct TokenResponse { access_token:Option<String>,refresh_token:Option<String>,expires_in:Option<u64> }
impl Drop for TokenResponse { fn drop(&mut self){use zeroize::Zeroize;if let Some(value)=&mut self.access_token{value.zeroize()}if let Some(value)=&mut self.refresh_token{value.zeroize()}} }
pub struct Tokens { pub access:Zeroizing<String>,pub refresh:Zeroizing<String>,pub expires_in:u64 }

fn parse_tokens(bytes:&[u8],existing_refresh:Option<&str>)->Result<Tokens,String>{
    let mut payload:TokenResponse=serde_json::from_slice(bytes).map_err(|_|"Réponse Google illisible.")?;
    let access=payload.access_token.take().filter(|value|!value.is_empty()&&value.len()<=8192&&!value.contains(['\r','\n'])).ok_or("Jeton Google manquant.")?;
    let refresh=payload.refresh_token.take().filter(|value|!value.is_empty()).or_else(||existing_refresh.map(str::to_owned)).filter(|value|!value.is_empty()&&value.len()<=8192&&!value.contains(['\r','\n'])).ok_or("Renouvellement Google indisponible.")?;
    let expires_in=payload.expires_in.filter(|value|(1..=86400).contains(value)).ok_or("Durée du jeton Google invalide.")?;
    Ok(Tokens{access:Zeroizing::new(access),refresh:Zeroizing::new(refresh),expires_in})
}

pub(crate) async fn exchange(client:&reqwest::Client,url:reqwest::Url,fields:&[(&str,&str)],existing_refresh:Option<&str>)->Result<Tokens,String>{
    let mut encoded=reqwest::Url::parse("https://localhost/").unwrap();
    {let mut query=encoded.query_pairs_mut();for(key,value) in fields {query.append_pair(key,value);}}
    let body=Zeroizing::new(encoded.query().unwrap_or_default().to_owned());
    tokio::time::timeout(Duration::from_secs(15),async{
        let mut response=client.post(url).header("content-type","application/x-www-form-urlencoded").body(body.to_string()).send().await.map_err(|_|"Connexion à Google impossible.")?;
        if !response.status().is_success(){return Err(format!("Google a refusé la connexion (HTTP {}).",response.status().as_u16()))}
        let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse Google interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse Google trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
        parse_tokens(&bytes,existing_refresh)
    }).await.map_err(|_|"Connexion Google expirée.".to_string())?
}

pub async fn refresh(client_id:&str,client_secret:&str,refresh_token:&str)->Result<Tokens,String>{
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    exchange(&client,reqwest::Url::parse("https://oauth2.googleapis.com/token").unwrap(),&[("grant_type","refresh_token"),("client_id",client_id),("client_secret",client_secret),("refresh_token",refresh_token)],Some(refresh_token)).await
}

#[derive(Debug,PartialEq)]
pub struct Identity { pub id:String, pub email:String }
fn parse_identity(bytes:&[u8])->Result<Identity,String>{
    let data:serde_json::Value=serde_json::from_slice(bytes).map_err(|_|"Identité Google illisible.")?;
    let id=data.get("id").and_then(|v|v.as_str()).filter(|v|!v.is_empty()&&v.len()<=256&&v.bytes().all(|c|c.is_ascii_alphanumeric()||matches!(c,b'-'|b'_'))).ok_or("Identifiant Google manquant.")?;
    let email=data.get("email").and_then(|v|v.as_str()).filter(|v|v.len()<=320&&v.contains('@')&&!v.chars().any(|c|c.is_whitespace()||c.is_control())).ok_or("Adresse Google manquante.")?;
    Ok(Identity{id:id.to_owned(),email:email.to_ascii_lowercase()})
}
async fn identity_request(client:&reqwest::Client,url:reqwest::Url,access:&str)->Result<Identity,String>{
    if access.is_empty()||access.contains(['\r','\n']){return Err("Connexion Google invalide.".into())}
    tokio::time::timeout(Duration::from_secs(15),async{
        let mut response=client.get(url).bearer_auth(access).send().await.map_err(|_|"Impossible de joindre Google.")?;
        if !response.status().is_success(){return Err(format!("Lecture de l'identité Google refusée (HTTP {}).",response.status().as_u16()))}
        let mut bytes=Zeroizing::new(Vec::new());while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture de l'identité interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse Google trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
        parse_identity(&bytes)
    }).await.map_err(|_|"Lecture de l'identité Google expirée.".to_string())?
}
pub async fn identity(access:&str)->Result<Identity,String>{
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    identity_request(&client,reqwest::Url::parse("https://www.googleapis.com/oauth2/v2/userinfo").unwrap(),access).await
}

#[cfg(test)] mod tests{
    use super::*;
    use bytes::Bytes;use http_body_util::{BodyExt,Full};use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;
    #[test] fn preserves_only_the_selected_refresh_token(){
        let token=parse_tokens(br#"{"access_token":"new-a","expires_in":3600}"#,Some("refresh-a")).unwrap();assert_eq!(token.access.as_str(),"new-a");assert_eq!(token.refresh.as_str(),"refresh-a");assert_eq!(token.expires_in,3600);
        let rotated=parse_tokens(br#"{"access_token":"new-a","refresh_token":"rotated-a","expires_in":60}"#,Some("refresh-a")).unwrap();assert_eq!(rotated.refresh.as_str(),"rotated-a");
        assert!(parse_tokens(br#"{"access_token":"a","expires_in":3600}"#,None).is_err());assert!(parse_tokens(br#"{"access_token":"a","expires_in":0}"#,Some("r")).is_err());
    }
    #[tokio::test] async fn refresh_exchange_uses_form_fields_and_never_returns_another_account(){
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send(body).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"access_token":"access-a","expires_in":3600}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let token=exchange(&client,format!("http://{address}/token").parse().unwrap(),&[("grant_type","refresh_token"),("refresh_token","refresh-a")],Some("refresh-a")).await.unwrap();assert_eq!(token.refresh.as_str(),"refresh-a");let body=rx.await.unwrap();assert!(std::str::from_utf8(&body).unwrap().contains("refresh_token=refresh-a"));
    }
    #[test] fn identity_requires_both_verified_id_and_email(){
        assert_eq!(parse_identity(br#"{"id":"12345","email":"Alice@Example.com"}"#).unwrap(),Identity{id:"12345".into(),email:"alice@example.com".into()});
        assert!(parse_identity(br#"{"email":"alice@example.com"}"#).is_err());
        assert!(parse_identity(br#"{"id":"12345","email":"not-an-email"}"#).is_err());
    }
    #[tokio::test] async fn identity_lookup_uses_access_token_of_this_account(){
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();sender.lock().unwrap().take().unwrap().send(auth).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"id":"123","email":"alice@example.com"}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let identity=identity_request(&client,format!("http://{address}/userinfo").parse().unwrap(),"synthetic-a").await.unwrap();assert_eq!(identity.id,"123");assert_eq!(rx.await.unwrap(),"Bearer synthetic-a");
    }
}
