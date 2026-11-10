//! Kiro Builder ID inference transport. Credentials are sent only to the fixed AWS endpoint.
use std::{collections::BTreeSet,time::Duration};
use serde_json::Value;

const ENDPOINT:&str="https://codewhisperer.us-east-1.amazonaws.com/generateAssistantResponse";
const CATALOG:&str="https://codewhisperer.us-east-1.amazonaws.com/ListAvailableModels";
#[derive(Debug)]pub struct GenerateError{pub status:u16,pub message:String,pub retry_after:Option<u32>}
impl From<&str> for GenerateError{fn from(message:&str)->Self{Self{status:502,message:message.into(),retry_after:None}}}
impl From<String> for GenerateError{fn from(message:String)->Self{let status=match message.as_str(){"__kiro_rate_limit"=>Some(429),"__kiro_forbidden"=>Some(403),"__kiro_invalid"=>Some(400),"__kiro_unavailable"=>Some(503),_=>None};if let Some(status)=status{upstream_error(status,None)}else{Self{status:502,message,retry_after:None}}}}
fn upstream_error(status:u16,retry:Option<&str>)->GenerateError{
 let (status,message)=match status{400=>(400,"Kiro a refusé la requête ou le modèle."),401=>(502,"La connexion Kiro a expiré ou a été refusée. Reconnecte le compte."),402=>(402,"Kiro indique que le crédit ou le quota est insuffisant."),403=>(403,"Ce compte Kiro n’a pas accès à cette demande."),429=>(429,"La limite de requêtes Kiro est atteinte."),503=>(503,"Kiro est temporairement indisponible."),_=>(502,"Kiro a refusé la demande.")};let retry_after=retry.and_then(|s|s.parse::<u32>().ok()).filter(|v|*v>0&&*v<=86400);GenerateError{status,message:message.into(),retry_after}
}

pub async fn catalog(token:&str)->Result<Vec<Value>,String>{
 let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(30)).build().map_err(|_|"Connexion Kiro indisponible.")?;
 catalog_at(&client,CATALOG.parse().expect("fixed Kiro endpoint"),token).await
}

pub(crate) async fn catalog_at(client:&reqwest::Client,url:reqwest::Url,token:&str)->Result<Vec<Value>,String>{
 let mut models=Vec::new();let mut seen=BTreeSet::new();let mut cursors=BTreeSet::new();let mut next=None::<String>;
 loop{
  let mut page=url.clone();{let mut query=page.query_pairs_mut();query.append_pair("origin","AI_EDITOR").append_pair("maxResults","50");if let Some(cursor)=&next{query.append_pair("nextToken",cursor);}}
  let mut response=client.get(page).bearer_auth(token).header("accept","application/json").send().await.map_err(|_|"Impossible de joindre Kiro.")?;
  if !response.status().is_success(){return Err(format!("Kiro refuse la liste des modèles (HTTP {}).",response.status().as_u16()))}
  let mut body=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture des modèles Kiro interrompue.")?{if body.len().saturating_add(chunk.len())>2*1024*1024{return Err("Liste des modèles Kiro trop volumineuse.".into())}body.extend_from_slice(&chunk);}
  let page:Value=serde_json::from_slice(&body).map_err(|_|"Liste des modèles Kiro illisible.")?;
  let entries=page.get("models").and_then(Value::as_array).ok_or("Liste des modèles Kiro invalide.")?;
  for entry in entries{let id=entry.get("modelId").and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=200&&!s.chars().any(|c|c.is_control()||c.is_whitespace())).ok_or("Identifiant de modèle Kiro invalide.")?;if seen.insert(id.to_owned()){let mut model=serde_json::json!({"id":id,"object":"model"});if let Some(schema)=entry.get("additionalModelRequestFieldsSchema").filter(|v|v.is_object()||v.is_string()){model["additionalModelRequestFieldsSchema"]=schema.clone()}models.push(model);if models.len()>500{return Err("Trop de modèles Kiro.".into())}}}
  next=match page.get("nextToken"){None|Some(Value::Null)=>None,Some(Value::String(s)) if s.is_empty()=>None,Some(Value::String(s)) if s.len()<=4096&&!s.chars().any(char::is_control)=>Some(s.clone()),_=>return Err("Pagination Kiro invalide.".into())};
  match &next{None=>break,Some(cursor) if cursors.len()<20&&cursors.insert(cursor.clone())=>{},_=>return Err("Pagination Kiro répétée ou trop longue.".into())}
 }
 Ok(models)
}

pub async fn generate(token:&str,request:&Value,model:String,allowed:BTreeSet<String>)->Result<Value,GenerateError>{
 let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(180)).build().map_err(|_|"Connexion Kiro indisponible.")?;
 generate_at(&client,ENDPOINT.parse().expect("fixed Kiro endpoint"),token,request,model,allowed).await
}

pub(crate) async fn generate_at(client:&reqwest::Client,url:reqwest::Url,token:&str,request:&Value,model:String,allowed:BTreeSet<String>)->Result<Value,GenerateError>{
 let mut response=client.post(url).bearer_auth(token).header("accept","application/vnd.amazon.eventstream").header("content-type","application/json").body(request.to_string()).send().await.map_err(|_|"Impossible de joindre Kiro.")?;
 if !response.status().is_success(){return Err(upstream_error(response.status().as_u16(),response.headers().get(reqwest::header::RETRY_AFTER).and_then(|v|v.to_str().ok())))}
 let content_type=response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v|v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").trim();
 if !content_type.eq_ignore_ascii_case("application/vnd.amazon.eventstream"){return Err("Format de réponse Kiro inattendu.".into())}
 let mut result=crate::kiro_response::Response::new(model,allowed);let mut bytes=0usize;
 while let Some(chunk)=response.chunk().await.map_err(|_|"Réponse Kiro interrompue.")?{bytes=bytes.saturating_add(chunk.len());if bytes>16*1024*1024{return Err("Réponse Kiro trop volumineuse.".into())}result.ingest(&chunk)?;}
 result.finish().map_err(Into::into)
}

#[cfg(test)]mod tests{
 use super::*;use bytes::Bytes;use http_body_util::{BodyExt,Full};use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;use std::convert::Infallible;use serde_json::json;
 #[test]fn distinguishes_rate_limit_from_connection_failure(){let rate=upstream_error(429,Some("120"));assert_eq!(rate.status,429);assert_eq!(rate.retry_after,Some(120));assert!(rate.message.contains("limite"));let auth=upstream_error(401,None);assert_eq!(auth.status,502);assert!(auth.message.contains("Reconnecte"));assert_eq!(upstream_error(500,Some("999999999")).retry_after,None);}
 #[tokio::test]async fn maps_streamed_throttling_exception(){
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(|_request:Request<Incoming>|async move{let wire=crate::kiro_eventstream::tests::frame_headers(&[(":message-type","exception"),(":exception-type","ThrottlingException")],br#"{"message":"private upstream detail"}"#);Ok::<_,Infallible>(Response::builder().header("content-type","application/vnd.amazon.eventstream").body(Full::new(Bytes::from(wire))).unwrap())});hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await.unwrap();});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let error=generate_at(&client,format!("http://{addr}/").parse().unwrap(),"synthetic",&json!({}),"m".into(),BTreeSet::new()).await.unwrap_err();assert_eq!(error.status,429);assert!(!error.message.contains("private"));
 }
 #[tokio::test]async fn sends_request_and_decodes_binary_response(){
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=std::sync::Mutex::new(Some(tx));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.lock().unwrap().take();async move{let(path,auth)=(request.uri().path().to_owned(),request.headers().get("authorization").unwrap().to_str().unwrap().to_owned());let body=request.into_body().collect().await.unwrap().to_bytes();tx.unwrap().send((path,auth,body)).unwrap();let mut wire=crate::kiro_eventstream::tests::frame("initial-response",br#"{"conversationId":""}"#);wire.extend(crate::kiro_eventstream::tests::frame("assistantResponseEvent",br#"{"content":"Bonjour"}"#));wire.extend(crate::kiro_eventstream::tests::frame("metadataEvent",br#"{"stopReason":"END_TURN"}"#));Ok::<_,Infallible>(Response::builder().header("content-type","application/vnd.amazon.eventstream").body(Full::new(Bytes::from(wire))).unwrap())}});hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await.unwrap();});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let payload=json!({"conversationState":{"history":[]}});let result=generate_at(&client,format!("http://{addr}/generateAssistantResponse").parse().unwrap(),"synthetic-token",&payload,"kiro/model".into(),BTreeSet::new()).await.unwrap();assert_eq!(result["choices"][0]["message"]["content"],"Bonjour");let(path,auth,body)=rx.await.unwrap();assert_eq!(path,"/generateAssistantResponse");assert_eq!(auth,"Bearer synthetic-token");assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(),payload);
 }
 #[tokio::test]async fn fetches_account_catalog_with_pagination(){
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,mut rx)=tokio::sync::mpsc::channel(2);
  tauri::async_runtime::spawn(async move{for i in 0..2{let(socket,_)=listener.accept().await.unwrap();let tx=tx.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{tx.send((request.uri().to_string(),request.headers().get("authorization").unwrap().to_str().unwrap().to_owned())).await.unwrap();let body=if i==0{br#"{"models":[{"modelId":"model-a","additionalModelRequestFieldsSchema":{"properties":{"max_tokens":{"type":"integer","minimum":1024,"maximum":128000}}}}],"nextToken":"cursor+1"}"#.as_slice()}else{br#"{"models":[{"modelId":"model-a"},{"modelId":"model-b"}]}"#.as_slice()};Ok::<_,Infallible>(Response::builder().header("content-type","application/json").body(Full::new(Bytes::copy_from_slice(body))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
  let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let models=catalog_at(&client,format!("http://{addr}/ListAvailableModels").parse().unwrap(),"synthetic-token").await.unwrap();assert_eq!(models,vec![json!({"id":"model-a","object":"model","additionalModelRequestFieldsSchema":{"properties":{"max_tokens":{"type":"integer","minimum":1024,"maximum":128000}}}}),json!({"id":"model-b","object":"model"})]);let(first,auth)=rx.recv().await.unwrap();let(second,_)=rx.recv().await.unwrap();assert!(first.contains("origin=AI_EDITOR"));assert!(second.contains("nextToken=cursor%2B1"));assert_eq!(auth,"Bearer synthetic-token");
 }
}
