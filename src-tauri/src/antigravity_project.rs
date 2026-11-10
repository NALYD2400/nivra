//! Read a Cloud Code Assist project for the supplied account.
use serde_json::{json,Value};
use std::time::Duration;

fn project_from_response(value:&Value)->Result<String,String>{
    let object=value.as_object().ok_or("Réponse de projet Antigravity invalide.")?;
    for key in ["cloudaicompanionProject","projectId","project"] {
        let Some(entry)=object.get(key) else {continue};
        let candidate=entry.as_str().or_else(||entry.get("id").and_then(Value::as_str));
        if let Some(id)=candidate.filter(|id|{
            let bare=id.strip_prefix("projects/").unwrap_or(id);
            !bare.is_empty()&&id.len()<=256&&bare.bytes().all(|byte|byte.is_ascii_alphanumeric()||matches!(byte,b'-'|b'_'|b'.'))
        }) {return Ok(id.to_owned())}
    }
    Err("Projet Antigravity introuvable pour ce compte.".into())
}

async fn request_project(client:&reqwest::Client,url:reqwest::Url,access_token:&str)->Result<Option<String>,String>{
    if access_token.is_empty()||access_token.contains(['\r','\n']){return Err("Connexion Antigravity invalide.".into())}
    tokio::time::timeout(Duration::from_secs(15),async{
        let mut response=client.post(url).bearer_auth(access_token).header("content-type","application/json").body(json!({"metadata":{"ideType":"ANTIGRAVITY"}}).to_string()).send().await.map_err(|_|"Impossible de joindre Google.")?;
        if !response.status().is_success(){return Err(format!("Lecture du projet refusée par Google (HTTP {}).",response.status().as_u16()))}
        let mut data=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture du projet interrompue.")?{if data.len()+chunk.len()>65536{return Err("Réponse de projet trop volumineuse.".into())}data.extend_from_slice(&chunk)}
        let value:Value=serde_json::from_slice(&data).map_err(|_|"Réponse de projet illisible.")?;
        if !value.is_object(){return Err("Réponse de projet Antigravity invalide.".into())}
        match project_from_response(&value){Ok(project)=>Ok(Some(project)),Err(error)=>{
            if ["cloudaicompanionProject","projectId","project"].iter().any(|key|value.get(*key).is_some_and(|v|!v.is_null()&&v.as_str()!=Some(""))){return Err(error)}
            Ok(None)
        }}
    }).await.map_err(|_|"Lecture du projet expirée.".to_string())?
}

pub async fn discover(access_token:&str)->Result<String,String>{
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    discover_at(&client,reqwest::Url::parse("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist").unwrap(),reqwest::Url::parse("https://daily-cloudcode-pa.googleapis.com/v1internal:onboardUser").unwrap(),access_token).await
}

async fn discover_at(client:&reqwest::Client,load:reqwest::Url,onboarding:reqwest::Url,access_token:&str)->Result<String,String>{
    if let Some(project)=request_project(client,load,access_token).await?{return Ok(project)}
    onboard(client,onboarding,access_token).await
}

/// Interpret the onboarding response without confusing pending or failed operations with a project.
fn onboarding_response(value:&Value)->Result<Option<String>,String>{
    let object=value.as_object().ok_or("Réponse d'initialisation Antigravity invalide.")?;
    match object.get("done").and_then(Value::as_bool){
        Some(false)=>Ok(None),
        Some(true)=>{
            if object.get("error").is_some_and(|error|!error.is_null()){return Err("Google n'a pas initialisé ce compte Antigravity.".into())}
            let body=object.get("response").ok_or("Projet Antigravity absent après initialisation.")?;
            Ok(Some(project_from_response(body)?))
        }
        None=>Err("Réponse d'initialisation Antigravity invalide.".into()),
    }
}

async fn onboard(client:&reqwest::Client,url:reqwest::Url,token:&str)->Result<String,String>{
    if token.is_empty()||token.contains(['\r','\n']){return Err("Connexion Antigravity invalide.".into())}
    tokio::time::timeout(Duration::from_secs(30),async{
        for attempt in 0..5 {
            let mut response=client.post(url.clone()).bearer_auth(token).header("content-type","application/json").body(json!({"tier_id":"free-tier","metadata":{"ide_type":"ANTIGRAVITY","ide_name":"antigravity"}}).to_string()).send().await.map_err(|_|"Initialisation Antigravity impossible.")?;
            let status=response.status();
            if status.is_success(){
                let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture Antigravity interrompue.")?{if bytes.len()+chunk.len()>65536{return Err("Réponse Antigravity trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}
                let value:Value=serde_json::from_slice(&bytes).map_err(|_|"Réponse Antigravity illisible.")?;
                if let Some(project)=onboarding_response(&value)?{return Ok(project)}
            }else if status.as_u16()!=429&&!status.is_server_error(){return Err(format!("Initialisation refusée par Google (HTTP {}).",status.as_u16()))}
            if attempt<4{tokio::time::sleep(Duration::from_secs(2)).await}
        }
        Err("L'initialisation Antigravity n'est pas terminée. Réessaie plus tard.".into())
    }).await.map_err(|_|"Initialisation Antigravity expirée.".to_string())?
}

#[cfg(test)] mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt,Full};
    use hyper::{body::Incoming,service::service_fn,Request,Response};
    use hyper_util::rt::TokioIo;
    #[tokio::test] async fn discovers_project_or_initializes_only_when_missing(){
        for (load_body,status,expected_calls,expected) in [
            (r#"{"currentTier":{"id":"free-tier"}}"#,200,2,Some("initialized-project")),
            (r#"{"cloudaicompanionProject":""}"#,200,2,Some("initialized-project")),
            (r#"{"cloudaicompanionProject":"existing-project"}"#,200,1,Some("existing-project")),
            (r#"{}"#,403,1,None),
            (r#"{"projectId":"bad/project"}"#,200,1,None),
        ]{
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
            let calls=std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));let observed=calls.clone();
            let task=tauri::async_runtime::spawn(async move{loop{let(socket,_)=listener.accept().await.unwrap();let calls=calls.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let calls=calls.clone();async move{
                assert_eq!(request.headers()["authorization"],"Bearer selected-account-token");
                let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();
                calls.lock().unwrap().push(path.clone());
                let (code,reply)=if path=="/load"{(status,load_body)}else{assert_eq!(path,"/onboard");let body:Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["tier_id"],"free-tier");(200,r#"{"done":true,"response":{"cloudaicompanionProject":"initialized-project"}}"#)};
                Ok::<_,std::convert::Infallible>(Response::builder().status(code).body(Full::new(Bytes::from(reply))).unwrap())
            }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
            let client=reqwest::Client::builder().no_proxy().build().unwrap();
            let result=discover_at(&client,format!("http://{address}/load").parse().unwrap(),format!("http://{address}/onboard").parse().unwrap(),"selected-account-token").await;
            assert_eq!(result.ok().as_deref(),expected);assert_eq!(observed.lock().unwrap().len(),expected_calls);task.abort();
        }
    }
    #[test] fn accepts_project_shapes_and_rejects_invalid_values(){
        assert_eq!(project_from_response(&json!({"cloudaicompanionProject":{"id":"project-123"}})).unwrap(),"project-123");
        assert_eq!(project_from_response(&json!({"projectId":"other-123"})).unwrap(),"other-123");
        assert_eq!(project_from_response(&json!({"cloudaicompanionProject":"projects/123456789"})).unwrap(),"projects/123456789");
        assert!(project_from_response(&json!({"projectId":"projects/"})).is_err());
        assert!(project_from_response(&json!({"projectId":"projects/one/other"})).is_err());
        assert!(project_from_response(&json!({"projectId":"\ninvalid"})).is_err());assert!(project_from_response(&json!({})).is_err());
    }
    #[test] fn onboarding_requires_completion_and_a_valid_project(){
        assert_eq!(onboarding_response(&json!({"done":false})).unwrap(),None);
        assert_eq!(onboarding_response(&json!({"done":true,"response":{"projectId":"project-123"}})).unwrap(),Some("project-123".into()));
        assert!(onboarding_response(&json!({"done":true,"error":{"message":"failure"},"response":{"projectId":"project-123"}})).is_err());
        assert!(onboarding_response(&json!({"done":true})).is_err());
    }
    #[tokio::test] async fn sends_only_the_selected_account_token_to_project_service(){
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,body)).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"projectId":"project-123"}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();let project=request_project(&client,format!("http://{address}/load").parse().unwrap(),"synthetic-account-a").await.unwrap();assert_eq!(project,Some("project-123".into()));let(auth,body)=rx.await.unwrap();assert_eq!(auth,"Bearer synthetic-account-a");let body:Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["metadata"]["ideType"],"ANTIGRAVITY");
    }
    #[tokio::test] async fn onboarding_uses_account_token_and_does_not_accept_failed_operation(){
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let sender=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,body)).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"done":true,"response":{"projectId":"project-123"}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();assert_eq!(onboard(&client,format!("http://{address}/onboard").parse().unwrap(),"account-a").await.unwrap(),"project-123");let(auth,body)=rx.await.unwrap();assert_eq!(auth,"Bearer account-a");let body:Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["metadata"]["ide_type"],"ANTIGRAVITY");
    }
}
