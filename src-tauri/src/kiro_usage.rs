//! Read the account's Kiro usage without inventing absent quota values.
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Serialize,Debug,PartialEq)]pub struct Usage{plan:Option<String>,reset_ms:Option<u64>,windows:Vec<Window>}
#[derive(Serialize,Debug,PartialEq)]struct Window{kind:String,used:Option<f64>,limit:Option<f64>,overage:Option<f64>}
fn amount(row:&Value,precise:&str,plain:&str)->Option<f64>{row.get(precise).or_else(||row.get(plain)).and_then(Value::as_f64).filter(|v|v.is_finite()&&*v>=0.0)}
fn parse(value:&Value)->Result<Usage,String>{
 let rows=value.get("usageBreakdownList").and_then(Value::as_array).filter(|rows|rows.len()<=32).ok_or("Réponse des quotas Kiro invalide.")?;
 let mut windows=Vec::new();for row in rows{let kind=row.get("resourceType").and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=100&&!s.chars().any(char::is_control)).ok_or("Type de quota Kiro invalide.")?;
  let used=amount(row,"currentUsageWithPrecision","currentUsage");let limit=amount(row,"usageLimitWithPrecision","usageLimit");let overage=amount(row,"currentOveragesWithPrecision","currentOverages");if used.is_some()||limit.is_some()||overage.is_some(){windows.push(Window{kind:kind.into(),used,limit,overage});}
  if let Some(trial)=row.get("freeTrialInfo"){let used=amount(trial,"currentUsageWithPrecision","currentUsage");let limit=amount(trial,"usageLimitWithPrecision","usageLimit");if used.is_some()||limit.is_some(){windows.push(Window{kind:format!("{kind} · essai"),used,limit,overage:None});}}
 }
 let plan=value.pointer("/subscriptionInfo/subscriptionTitle").and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=200&&!s.chars().any(char::is_control)).map(str::to_owned);
 let reset_ms=value.get("nextDateReset").and_then(Value::as_f64).filter(|n|n.is_finite()&&*n>=1.0).and_then(|n|{let ms=if n<10_000_000_000.0{n*1000.0}else{n};(ms<253_402_300_800_000.0).then_some(ms as u64)});
 Ok(Usage{plan,reset_ms,windows})
}
#[tauri::command]pub async fn kiro_builder_usage(credential_ref:String)->Result<Usage,String>{
 let token=crate::kiro_login::access(&credential_ref).await?;let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(30)).build().map_err(|_|"Connexion Kiro indisponible.")?;
 fetch(&client,"https://codewhisperer.us-east-1.amazonaws.com/getUsageLimits".parse().unwrap(),token.as_str()).await
}
async fn fetch(client:&reqwest::Client,mut url:reqwest::Url,token:&str)->Result<Usage,String>{
 url.query_pairs_mut().append_pair("origin","AI_EDITOR").append_pair("resourceType","AGENTIC_REQUEST");
 let mut response=client.get(url).bearer_auth(token).header("accept","application/json").send().await.map_err(|_|"Impossible de lire les quotas Kiro.")?;if !response.status().is_success(){return Err(format!("Lecture des quotas Kiro refusée (HTTP {}).",response.status().as_u16()))}
 let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture des quotas Kiro interrompue.")?{if bytes.len().saturating_add(chunk.len())>1024*1024{return Err("Réponse Kiro trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}parse(&serde_json::from_slice(&bytes).map_err(|_|"Réponse des quotas Kiro illisible.")?)
}
#[cfg(test)]mod tests{use super::*;use serde_json::json;
 #[test]fn keeps_precise_plan_and_trial_separate(){let result=parse(&json!({"subscriptionInfo":{"subscriptionTitle":"KIRO PRO"},"nextDateReset":1800000000,"usageBreakdownList":[{"resourceType":"AGENTIC_REQUEST","currentUsage":8,"currentUsageWithPrecision":8.5,"usageLimitWithPrecision":50,"currentOveragesWithPrecision":1.5,"freeTrialInfo":{"currentUsage":2,"usageLimit":10}}]})).unwrap();assert_eq!(result.plan.as_deref(),Some("KIRO PRO"));assert_eq!(result.reset_ms,Some(1800000000000));assert_eq!(result.windows[0].used,Some(8.5));assert_eq!(result.windows[0].overage,Some(1.5));assert_eq!(result.windows[1].limit,Some(10.0));}
 #[test]fn missing_values_are_not_zero(){let result=parse(&json!({"usageBreakdownList":[{"resourceType":"AGENTIC_REQUEST","usageLimit":50}]})).unwrap();assert_eq!(result.windows[0].used,None);assert_eq!(result.reset_ms,None);assert!(parse(&json!({})).is_err());}
 #[tokio::test]async fn fetches_account_limits(){
  use bytes::Bytes;use http_body_util::Full;use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;use std::convert::Infallible;
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
  tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=std::sync::Mutex::new(Some(tx));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.lock().unwrap().take();async move{tx.unwrap().send((request.uri().to_string(),request.headers().get("authorization").unwrap().to_str().unwrap().to_owned())).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"usageBreakdownList":[{"resourceType":"AGENTIC_REQUEST","currentUsageWithPrecision":1.5,"usageLimitWithPrecision":50}]}"#))))}});hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await.unwrap();});
  let client=reqwest::Client::builder().no_proxy().build().unwrap();let result=fetch(&client,format!("http://{addr}/getUsageLimits").parse().unwrap(),"synthetic-token").await.unwrap();assert_eq!(result.windows[0].used,Some(1.5));let(uri,auth)=rx.await.unwrap();assert!(uri.contains("origin=AI_EDITOR"));assert!(uri.contains("resourceType=AGENTIC_REQUEST"));assert_eq!(auth,"Bearer synthetic-token");
 }
}
