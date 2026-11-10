//! Local, authenticated forwarding for OpenAI-compatible APIs.
//! Credentials exist only in memory and are never returned by the data plane.
use std::{collections::{BTreeMap, VecDeque}, convert::Infallible, net::{IpAddr, Ipv4Addr, SocketAddr}, sync::{Arc, Mutex, RwLock}, time::Duration};
use bytes::Bytes;
use futures_util::stream;
use http_body_util::{BodyExt, Full, Limited, StreamBody, combinators::UnsyncBoxBody};
use hyper::{body::{Frame, Incoming}, service::service_fn, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::{net::TcpListener, sync::{watch, Semaphore}};
use zeroize::Zeroize;

type ResponseBody = UnsyncBoxBody<Bytes, std::io::Error>;
const MAX_REQUEST: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub struct GatewayManager(Mutex<Option<Running>>);
struct Running { shutdown: watch::Sender<bool>, status: GatewayStatus, router: Arc<Router> }
#[derive(Clone, Serialize)]
pub struct GatewayStatus { pub address: String, pub token: String, pub base_url: String }
struct Upstream { base: reqwest::Url, key: String, client: reqwest::Client, chat_only: bool, anthropic_native: bool, antigravity:Option<(String,String)>, kimi:Option<String>, xai:Option<String>, copilot:Option<String>, chatgpt:Option<String>, qwen:Option<String>, nous:Option<String>, kiro:Option<String> }
impl Drop for Upstream { fn drop(&mut self) { self.key.zeroize(); } }
struct Router { token: String, default_id: String, providers: RwLock<BTreeMap<String, Arc<Upstream>>> }

#[derive(Clone, Serialize)]
pub struct ProviderInfo { pub id: String, pub base_url: String, pub is_default: bool, pub chat_only: bool, pub anthropic_native: bool, pub antigravity:bool }

#[derive(Serialize,Debug,PartialEq)]
pub struct OpenRouterUsage { limit:Option<f64>, remaining:Option<f64>, usage:Option<f64>, daily:Option<f64>, weekly:Option<f64>, monthly:Option<f64>, reset:Option<String> }

#[derive(Serialize,Debug,PartialEq)]
pub struct DeepSeekBalance { available:bool, balances:Vec<DeepSeekAmount> }
#[derive(Serialize,Debug,PartialEq)]
struct DeepSeekAmount { currency:String, total:String, granted:Option<String>, topped_up:Option<String> }

#[derive(Serialize,Debug,PartialEq)]
pub struct OpenCodeGoUsage{windows:Vec<OpenCodeGoWindow>}
#[derive(Serialize,Debug,PartialEq)]
struct OpenCodeGoWindow{kind:String,status:String,percent_used:f64,reset:String}
fn is_opencode_go(upstream:&Upstream)->bool{upstream.base.as_str()=="https://opencode.ai/zen/go/v1/"&&upstream.antigravity.is_none()&&upstream.kimi.is_none()&&upstream.xai.is_none()&&upstream.copilot.is_none()&&upstream.qwen.is_none()&&upstream.nous.is_none()&&upstream.kiro.is_none()}
fn is_cline(upstream:&Upstream)->bool{
 let official=upstream.base.as_str()=="https://api.cline.bot/api/v1/";
 #[cfg(test)]let official=official||(upstream.base.host_str()==Some("127.0.0.1")&&upstream.base.path()=="/__nivra_test_cline/v1/");
 official&&upstream.antigravity.is_none()&&upstream.kimi.is_none()&&upstream.xai.is_none()&&upstream.copilot.is_none()&&upstream.chatgpt.is_none()&&upstream.qwen.is_none()&&upstream.nous.is_none()&&upstream.kiro.is_none()
}
#[derive(Serialize,Debug,PartialEq)]pub struct ClineUsage{windows:Vec<ClineWindow>}
#[derive(Serialize,Debug,PartialEq)]struct ClineWindow{kind:String,percent_used:f64,reset:Option<String>}
fn parse_cline_usage(value:&serde_json::Value)->Result<ClineUsage,String>{
 if value.get("success").and_then(|v|v.as_bool())!=Some(true){return Err("Cline n’a pas communiqué les quotas de cet abonnement.".into())}
 let rows=value.pointer("/data/limits").and_then(|v|v.as_array()).filter(|v|v.len()<=32).ok_or("Réponse des quotas Cline illisible.")?;let mut windows=Vec::new();let mut seen=std::collections::BTreeSet::new();
 for row in rows{let Some(kind)=row.get("type").and_then(|v|v.as_str()).filter(|v|matches!(*v,"five_hour"|"weekly"|"monthly"))else{continue};if !seen.insert(kind){return Err("Fenêtre de quota Cline dupliquée.".into())}let percent=row.get("percentUsed").and_then(|v|v.as_f64()).filter(|v|v.is_finite()&&*v>=0.0).ok_or("Quota Cline invalide.")?;let reset=match row.get("resetsAt"){None|Some(serde_json::Value::Null)=>None,Some(value)=>Some(value.as_str().filter(|v|v.len()<=64&&!v.chars().any(char::is_control)).ok_or("Date de quota Cline invalide.")?.to_owned())};windows.push(ClineWindow{kind:kind.into(),percent_used:percent,reset});}Ok(ClineUsage{windows})
}
async fn fetch_cline_usage(client:&reqwest::Client,url:reqwest::Url,key:&str)->Result<ClineUsage,String>{
 tokio::time::timeout(Duration::from_secs(15),async{let mut response=client.get(url).bearer_auth(key).header("accept","application/json").send().await.map_err(|_|"Impossible de lire les quotas ClinePass.")?;if !response.status().is_success(){return Err(format!("Lecture des quotas ClinePass refusée (HTTP {}).",response.status().as_u16()))}let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture ClinePass interrompue.")?{if bytes.len().saturating_add(chunk.len())>65536{return Err("Réponse ClinePass trop volumineuse.".into())}bytes.extend_from_slice(&chunk)}parse_cline_usage(&serde_json::from_slice(&bytes).map_err(|_|"Réponse ClinePass illisible.")?)}).await.map_err(|_|"Lecture ClinePass expirée.".to_string())?
}
#[tauri::command]pub async fn gateway_cline_usage(manager:tauri::State<'_,GatewayManager>,provider_id:String)->Result<ClineUsage,String>{let router=current_router(&manager)?;let upstream=router.providers.read().map_err(|_|"Fournisseurs indisponibles.")?.get(&provider_id).cloned().ok_or("Fournisseur introuvable.")?;if !is_cline(&upstream)||upstream.key.is_empty(){return Err("Les quotas ClinePass nécessitent une clé et l’adresse officielle Cline.".into())}fetch_cline_usage(&upstream.client,"https://api.cline.bot/api/v1/users/me/plan/usage-limits".parse().unwrap(),&upstream.key).await}
fn cline_chat_body(body:&Bytes)->Result<Bytes,String>{let mut value:serde_json::Value=serde_json::from_slice(body).map_err(|_|"Requête Cline invalide.")?;let object=value.as_object_mut().ok_or("Requête Cline invalide.")?;match object.get("stream"){None=>{object.insert("stream".into(),serde_json::json!(false));},Some(v) if v.is_boolean()=>{},_=>return Err("Option de flux Cline invalide.".into())}Ok(Bytes::from(value.to_string()))}
fn parse_opencode_go_usage(value:&serde_json::Value)->Result<OpenCodeGoUsage,String>{
 let usage=value.get("usage").and_then(|v|v.as_object()).ok_or("Réponse OpenCode Go illisible.")?;let mut windows=Vec::new();
 for kind in ["rolling","weekly","monthly"]{if let Some(row)=usage.get(kind){
  let status=row.get("status").and_then(|v|v.as_str()).filter(|v|matches!(*v,"ok"|"rate-limited")).ok_or("État OpenCode Go inconnu.")?;
  let percent=row.get("percent").and_then(|v|v.as_f64()).filter(|v|v.is_finite()&&*v>=0.0).ok_or("Quota OpenCode Go invalide.")?;
  let reset=row.get("resetsAt").and_then(|v|v.as_str()).filter(|v|v.len()>=20&&v.len()<=40&&v.contains('T')&&(v.ends_with('Z')||v.rsplit('T').next().is_some_and(|s|s.contains('+')||s.contains('-')))&&v.bytes().all(|b|b.is_ascii_digit()||matches!(b,b'-'|b':'|b'T'|b'Z'|b'.'|b'+'))).ok_or("Date OpenCode Go invalide.")?;
  windows.push(OpenCodeGoWindow{kind:kind.into(),status:status.into(),percent_used:percent,reset:reset.into()});
 }}
 Ok(OpenCodeGoUsage{windows})
}
async fn fetch_opencode_go_usage(upstream:Arc<Upstream>,url:reqwest::Url)->Result<OpenCodeGoUsage,String>{
 tokio::time::timeout(Duration::from_secs(15),async move{
  let mut response=upstream.client.get(url).bearer_auth(&upstream.key).header("accept","application/json").send().await.map_err(|_|"Impossible de lire les quotas OpenCode Go.")?;
  if !response.status().is_success(){return Err(format!("Lecture des quotas OpenCode Go refusée (HTTP {}).",response.status().as_u16()))}let mut data=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture OpenCode Go interrompue.")?{if data.len()+chunk.len()>65536{return Err("Réponse OpenCode Go trop volumineuse.".into())}data.extend_from_slice(&chunk)}let value=serde_json::from_slice(&data).map_err(|_|"Réponse OpenCode Go illisible.")?;parse_opencode_go_usage(&value)
 }).await.map_err(|_|"La lecture OpenCode Go a expiré.".to_string())?
}
#[tauri::command] pub async fn gateway_opencode_go_usage(manager:tauri::State<'_,GatewayManager>,provider_id:String)->Result<OpenCodeGoUsage,String>{
 let router=current_router(&manager)?;let upstream=router.providers.read().map_err(|_|"Fournisseurs indisponibles.")?.get(&provider_id).cloned().ok_or("Fournisseur introuvable.")?;if !is_opencode_go(&upstream){return Err("La lecture est réservée à l’adresse officielle OpenCode Go.".into())}if upstream.key.is_empty(){return Err("Ajoute une clé OpenCode Go pour lire les quotas.".into())}fetch_opencode_go_usage(upstream,"https://opencode.ai/zen/go/v1/usage".parse().unwrap()).await
}

#[derive(Serialize,Debug,PartialEq)]
pub struct GlmQuota{windows:Vec<GlmWindow>}
#[derive(Serialize,Debug,PartialEq)]
struct GlmWindow{kind:String,unit:Option<u64>,number:Option<u64>,percent_used:Option<f64>,used:Option<f64>,limit:Option<f64>,remaining:Option<f64>,reset_ms:Option<u64>}
fn glm_region(upstream:&Upstream)->Option<&'static str>{
 if upstream.antigravity.is_some()||upstream.kimi.is_some()||upstream.xai.is_some()||upstream.copilot.is_some()||upstream.qwen.is_some()||upstream.nous.is_some()||upstream.kiro.is_some(){return None}
 match upstream.base.as_str(){"https://api.z.ai/"|"https://api.z.ai/api/coding/paas/v4/"|"https://api.z.ai/api/anthropic/"=>Some("zai"),"https://open.bigmodel.cn/"|"https://open.bigmodel.cn/api/coding/paas/v4/"|"https://open.bigmodel.cn/api/v1/"|"https://open.bigmodel.cn/api/anthropic/"=>Some("bigmodel"),_=>None}
}
fn parse_glm_quota(value:&serde_json::Value)->Result<GlmQuota,String>{
 if value.get("success").and_then(|v|v.as_bool())==Some(false){return Err("Le service GLM n’a pas communiqué les quotas.".into())}
 let rows=value.get("data").and_then(|v|v.get("limits")).and_then(|v|v.as_array()).filter(|v|v.len()<=32).ok_or("Réponse des quotas GLM illisible.")?;
 let mut windows=Vec::new();for row in rows{
  let Some(kind)=row.get("type").and_then(|v|v.as_str()).filter(|v|matches!(*v,"TOKENS_LIMIT"|"TIME_LIMIT")) else{continue};
  let n=|key:&str|row.get(key).and_then(|v|v.as_f64()).filter(|v|v.is_finite()&&*v>=0.0);
  let unit=row.get("unit").and_then(|v|v.as_u64()).filter(|v|*v<=100);let number=row.get("number").and_then(|v|v.as_u64()).filter(|v|*v<=10000);
  let percent=n("percentage");let used=n("currentValue");let limit=n("usage");let remaining=n("remaining");let reset_ms=row.get("nextResetTime").and_then(|v|v.as_u64()).filter(|v|*v>0&&*v<253402300800000);
  if percent.is_some()||used.is_some()||limit.is_some()||remaining.is_some()||reset_ms.is_some(){windows.push(GlmWindow{kind:kind.into(),unit,number,percent_used:percent,used,limit,remaining,reset_ms})}
 }
 Ok(GlmQuota{windows})
}
async fn fetch_glm_quota(upstream:Arc<Upstream>,url:reqwest::Url,region:&str)->Result<GlmQuota,String>{
 tokio::time::timeout(Duration::from_secs(15),async move{
  let mut request=upstream.client.get(url).header("accept","application/json");request=if region=="bigmodel"{request.header("authorization",&upstream.key)}else{request.bearer_auth(&upstream.key)};
  let mut response=request.send().await.map_err(|_|"Impossible de lire les quotas GLM.")?;if !response.status().is_success(){return Err(format!("Lecture des quotas GLM refusée (HTTP {}).",response.status().as_u16()))}
  let mut data=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture des quotas GLM interrompue.")?{if data.len()+chunk.len()>65536{return Err("Réponse GLM trop volumineuse.".into())}data.extend_from_slice(&chunk)}let value=serde_json::from_slice(&data).map_err(|_|"Réponse des quotas GLM illisible.")?;parse_glm_quota(&value)
 }).await.map_err(|_|"La lecture des quotas GLM a expiré.".to_string())?
}
#[tauri::command]
pub async fn gateway_glm_quota(manager:tauri::State<'_,GatewayManager>,provider_id:String)->Result<GlmQuota,String>{
 let router=current_router(&manager)?;let upstream=router.providers.read().map_err(|_|"Fournisseurs indisponibles.")?.get(&provider_id).cloned().ok_or("Fournisseur introuvable.")?;
 let region=glm_region(&upstream).ok_or("La lecture des quotas est réservée aux adresses officielles GLM Coding Plan.")?;if upstream.key.is_empty(){return Err("Ajoute une clé GLM Coding Plan pour lire les quotas.".into())}
 let url=if region=="zai"{"https://api.z.ai/api/monitor/usage/quota/limit"}else{"https://open.bigmodel.cn/api/monitor/usage/quota/limit"};fetch_glm_quota(upstream,url.parse().unwrap(),region).await
}

fn parse_deepseek_balance(value:&serde_json::Value)->Result<DeepSeekBalance,String>{
    let available=value.get("is_available").and_then(|value|value.as_bool()).ok_or("Réponse DeepSeek illisible.")?;
    let rows=value.get("balance_infos").and_then(|value|value.as_array()).filter(|rows|rows.len()<=20).ok_or("Réponse DeepSeek illisible.")?;
    fn amount(value:Option<&serde_json::Value>)->Option<String>{let value=value?.as_str()?;if value.is_empty()||value.len()>40||!value.bytes().all(|byte|byte.is_ascii_digit()||byte==b'.')||value.bytes().filter(|byte|*byte==b'.').count()>1||!value.bytes().any(|byte|byte.is_ascii_digit()){return None}Some(value.to_owned())}
    let mut balances=Vec::new();
    for row in rows {let Some(currency)=row.get("currency").and_then(|v|v.as_str()).filter(|v|matches!(*v,"USD"|"CNY")) else{continue};let Some(total)=amount(row.get("total_balance")) else{continue};balances.push(DeepSeekAmount{currency:currency.into(),total,granted:amount(row.get("granted_balance")),topped_up:amount(row.get("topped_up_balance"))});}
    if !rows.is_empty()&&balances.is_empty(){return Err("Solde DeepSeek illisible.".into())}
    Ok(DeepSeekBalance{available,balances})
}

fn is_official_deepseek(upstream:&Upstream)->bool{matches!(upstream.base.as_str(),"https://api.deepseek.com/"|"https://api.deepseek.com/v1/")&&!upstream.anthropic_native}

async fn fetch_deepseek_balance(upstream:Arc<Upstream>,url:reqwest::Url)->Result<DeepSeekBalance,String>{
    tokio::time::timeout(Duration::from_secs(12),async move {
        let mut response=upstream.client.get(url).bearer_auth(&upstream.key).send().await.map_err(|_|"Impossible de joindre DeepSeek.")?;
        if !response.status().is_success(){return Err(format!("DeepSeek a refusé la lecture (HTTP {}).",response.status().as_u16()))}
        let mut data=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture DeepSeek interrompue.")?{if data.len()+chunk.len()>65536{return Err("Réponse DeepSeek trop volumineuse.".into())}data.extend_from_slice(&chunk)}
        let value=serde_json::from_slice(&data).map_err(|_|"Réponse DeepSeek illisible.")?;parse_deepseek_balance(&value)
    }).await.map_err(|_|"La lecture DeepSeek a expiré.".to_string())?
}

#[tauri::command]
pub async fn gateway_deepseek_balance(manager:tauri::State<'_,GatewayManager>,provider_id:String)->Result<DeepSeekBalance,String>{
    let router=current_router(&manager)?;
    let upstream=router.providers.read().map_err(|_|"Fournisseurs indisponibles.")?.get(&provider_id).cloned().ok_or("Fournisseur introuvable.")?;
    if !is_official_deepseek(&upstream){return Err("La lecture du solde est réservée à l’adresse officielle DeepSeek.".into())}
    if upstream.key.is_empty(){return Err("Ajoute une clé DeepSeek pour lire son solde.".into())}
    fetch_deepseek_balance(upstream,reqwest::Url::parse("https://api.deepseek.com/user/balance").unwrap()).await
}

fn parse_openrouter_usage(value:&serde_json::Value)->Result<OpenRouterUsage,String>{
    let data=value.get("data").and_then(|value|value.as_object()).ok_or("Réponse OpenRouter illisible.")?;
    let number=|key:&str|data.get(key).and_then(|value|value.as_f64()).filter(|value|value.is_finite()&&*value>=0.0);
    let reset=data.get("limit_reset").and_then(|value|value.as_str()).filter(|value|matches!(*value,"daily"|"weekly"|"monthly")).map(str::to_owned);
    Ok(OpenRouterUsage{limit:number("limit"),remaining:number("limit_remaining"),usage:number("usage"),daily:number("usage_daily"),weekly:number("usage_weekly"),monthly:number("usage_monthly"),reset})
}

fn is_official_openrouter(upstream:&Upstream)->bool{upstream.base.as_str()=="https://openrouter.ai/api/v1/"&&!upstream.anthropic_native}

async fn fetch_openrouter_usage(upstream:Arc<Upstream>,url:reqwest::Url)->Result<OpenRouterUsage,String>{
    tokio::time::timeout(Duration::from_secs(12),async move {
        let mut response=upstream.client.get(url).bearer_auth(&upstream.key).send().await.map_err(|_|"Impossible de joindre OpenRouter.")?;
        if !response.status().is_success(){return Err(format!("OpenRouter a refusé la lecture (HTTP {}).",response.status().as_u16()))}
        let mut data=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture OpenRouter interrompue.")?{if data.len()+chunk.len()>65536{return Err("Réponse OpenRouter trop volumineuse.".into())}data.extend_from_slice(&chunk);}
        let value=serde_json::from_slice(&data).map_err(|_|"Réponse OpenRouter illisible.")?;parse_openrouter_usage(&value)
    }).await.map_err(|_|"La lecture OpenRouter a expiré.".to_string())?
}

#[tauri::command]
pub async fn gateway_openrouter_usage(manager:tauri::State<'_,GatewayManager>,provider_id:String)->Result<OpenRouterUsage,String>{
    let router=current_router(&manager)?;
    let upstream=router.providers.read().map_err(|_|"Fournisseurs indisponibles.")?.get(&provider_id).cloned().ok_or("Fournisseur introuvable.")?;
    if !is_official_openrouter(&upstream) {return Err("La lecture des limites est réservée à l’adresse officielle OpenRouter.".into())}
    if upstream.key.is_empty(){return Err("Ajoute une clé OpenRouter pour lire ses limites.".into())}
    let url=upstream.base.join("key").map_err(|_|"Adresse OpenRouter invalide.")?;
    fetch_openrouter_usage(upstream,url).await
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn copilot_responses_model(model:&str)->bool{
    matches!(model,"gpt-5.3-codex"|"gpt-5.4"|"gpt-5.4-mini"|"gpt-5.5"|"gpt-5.6-luna"|"gpt-5.6-sol"|"gpt-5.6-terra"|"gpt-6.1-sol"|"gpt-6.1-terra"|"gpt-6-astra"|"grok-4.5"|"grok-4.6"|"mai-code-1.1-flash"|"mai-code-1-flash-picker")
}
fn opencode_go_responses_model(model:&str)->bool{matches!(model,"grok-4.7"|"grok-4.6"|"gpt-6-luna"|"gpt-5.6-luna"|"muse-spark-1.3-contributor"|"muse-spark-1.2-contributor")}
fn opencode_go_messages_model(model:&str)->bool{matches!(model,"minimax-m3"|"minimax-m2.7"|"qwen3.8-max"|"qwen3.8-flash"|"qwen3.7-plus")}
fn anthropic_wire_for(upstream:&Upstream,model:Option<&str>)->bool{upstream.anthropic_native||(is_opencode_go(upstream)&&model.is_some_and(opencode_go_messages_model))}

fn create_upstream_with_mode(base_url: &str, api_key: String, chat_only: bool, anthropic_native:bool) -> Result<Arc<Upstream>, String> {
    if chat_only && anthropic_native { return Err("Protocoles incompatibles.".into()); }
    let base = upstream_url(base_url)?;
    if api_key.contains(['\r', '\n']) { return Err("Clé fournisseur invalide.".into()); }
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_| "Impossible de créer la connexion réseau.")?;
    Ok(Arc::new(Upstream { base, key: api_key, client, chat_only, anthropic_native,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None }))
}
#[cfg(test)]
fn create_upstream(base_url:&str,api_key:String,chat_only:bool)->Result<Arc<Upstream>,String>{create_upstream_with_mode(base_url,api_key,chat_only,false)}

fn provider_list(router: &Router) -> Result<Vec<ProviderInfo>, String> {
    Ok(router.providers.read().map_err(|_| "Fournisseurs indisponibles.")?.iter().map(|(id, upstream)| ProviderInfo {
        id: id.clone(), base_url: upstream.base.to_string(), is_default: id == &router.default_id, chat_only: upstream.chat_only, anthropic_native: upstream.anthropic_native,antigravity:upstream.antigravity.is_some(),
    }).collect())
}

fn select_provider(router: &Router, body: &Bytes) -> Result<(Arc<Upstream>, Bytes), String> {
    let mut json: serde_json::Value = serde_json::from_slice(body).map_err(|_| "Le corps doit être un objet JSON valide.")?;
    let model = json.get("model").and_then(|value| value.as_str()).filter(|model| !model.is_empty()).ok_or("La requête doit préciser un modèle.")?;
    let providers = router.providers.read().map_err(|_| "Fournisseurs indisponibles.")?;
    let selected = model.split_once('/').and_then(|(prefix, name)| providers.get(prefix).filter(|_| !name.is_empty()).map(|provider| (provider.clone(), name.to_string())));
    if let Some((provider, name)) = selected {
        json["model"] = serde_json::Value::String(name);
        return Ok((provider, Bytes::from(serde_json::to_vec(&json).map_err(|_| "Requête invalide.")?)));
    }
    if model.contains('/') {
        return Err("Préfixe de fournisseur inconnu. Utilise identifiant/modèle ; pour un modèle contenant /, ajoute aussi l’identifiant du fournisseur.".into());
    }
    let provider = providers.get(&router.default_id).ok_or("Fournisseur par défaut indisponible.")?.clone();
    Ok((provider, body.clone()))
}

fn antigravity_output_limit(value:Option<&serde_json::Value>)->Result<u32,String>{
    match value {None|Some(serde_json::Value::Null)=>Ok(2048),Some(value)=>value.as_u64().filter(|n|(1..=65536).contains(n)).map(|n|n as u32).ok_or_else(||"La limite de sortie Antigravity doit être un entier entre 1 et 65536.".into())}
}
fn simple_antigravity_chat(value:&serde_json::Value)->Result<(&str,serde_json::Value,bool,u32),String>{
    let object=value.as_object().ok_or("Requête Chat invalide.")?;
    if object.iter().any(|(key,value)|!matches!(key.as_str(),"model"|"messages"|"stream"|"max_tokens"|"max_completion_tokens"|"tools"|"tool_choice"|"temperature"|"top_p"|"stop")&&!value.is_null()){return Err("Cette fonction Chat n'est pas encore disponible pour Antigravity.".into())}
    if object.get("tools").is_some_and(|value|!value.is_null()&&value.as_array().is_none_or(|tools|!tools.is_empty())){return Err("Les outils ne sont pas encore disponibles pour Antigravity.".into())}
    if object.get("tool_choice").is_some_and(|value|!value.is_null()&&value.as_str()!=Some("none")){return Err("Le choix d'outil n'est pas encore disponible pour Antigravity.".into())}
    let streaming=match object.get("stream"){None=>false,Some(value)=>value.as_bool().ok_or("Option de flux invalide.")?};
    let model=object.get("model").and_then(|v|v.as_str()).filter(|s|!s.is_empty()).ok_or("Modèle manquant.")?;
    let messages=object.get("messages").and_then(|v|v.as_array()).filter(|messages|!messages.is_empty()&&messages.len()<=128).ok_or("Historique de 1 à 128 messages attendu.")?;
    let mut next="user"; let mut total=0usize; let mut started=false;
    for message in messages {
        let item=message.as_object().ok_or("Message invalide.")?;
        if item.keys().any(|key|!matches!(key.as_str(),"role"|"content")){return Err("Métadonnées et outils non pris en charge par Antigravity.".into())}
        let role=item.get("role").and_then(|v|v.as_str()).ok_or("Rôle manquant.")?;
        let content=item.get("content").ok_or("Contenu manquant.")?;
        if matches!(role,"system"|"developer") {let text=content.as_str().filter(|s|!s.trim().is_empty()).ok_or("Instructions texte requises.")?;total=total.checked_add(text.len()).ok_or("Historique trop volumineux.")?;}else{let parts=crate::google_request::content_parts(content,role)?;for part in parts{if let Some(text)=part.get("text").and_then(|v|v.as_str()){total=total.checked_add(text.len()).ok_or("Historique trop volumineux.")?;}}}
        if total>128_000{return Err("Historique trop volumineux.".into())}
        if matches!(role,"system"|"developer") && !started {continue}
        if role!=next{return Err("L'historique doit alterner utilisateur et assistant et finir par un message utilisateur.".into())}
        started=true;
        next=if next=="user"{"assistant"}else{"user"};
    }
    if next!="assistant"{return Err("L'historique doit finir par un message utilisateur.".into())}
    let mut converted=crate::google_request::request(&serde_json::json!({"model":model,"messages":messages}))?;
    crate::google_request::generation_options(value,&mut converted)?;
    let max_tokens=object.get("max_tokens").filter(|v|!v.is_null());
    let max_completion_tokens=object.get("max_completion_tokens").filter(|v|!v.is_null());
    if max_tokens.is_some()&&max_completion_tokens.is_some(){return Err("Indique une seule limite de sortie.".into())}
    Ok((model,converted,streaming,antigravity_output_limit(max_tokens.or(max_completion_tokens))?))
}

fn simple_antigravity_responses(value:&serde_json::Value)->Result<(&str,serde_json::Value,bool,u32),String>{
    let object=value.as_object().ok_or("Requête Responses invalide.")?;
    if object.iter().any(|(key,value)|!matches!(key.as_str(),"model"|"input"|"instructions"|"stream"|"store"|"tools"|"max_output_tokens"|"temperature"|"top_p")&&!value.is_null()){return Err("Cette fonction Responses n'est pas encore disponible pour Antigravity.".into())}
    if object.get("tools").is_some_and(|value|!value.is_null()&&value.as_array().is_none_or(|tools|!tools.is_empty())){return Err("Les outils ne sont pas encore disponibles pour Antigravity.".into())}
    if object.get("store").is_some_and(|value|!value.is_null()&&value.as_bool()!=Some(false)){return Err("Le stockage des réponses n'est pas pris en charge.".into())}
    let model=object.get("model").and_then(|v|v.as_str()).filter(|s|!s.is_empty()).ok_or("Modèle manquant.")?;
    let mut messages=Vec::new();
    if let Some(instructions)=object.get("instructions").filter(|v|!v.is_null()){
        let instructions=instructions.as_str().ok_or("Instructions invalides.")?;
        if !instructions.trim().is_empty(){messages.push(serde_json::json!({"role":"system","content":instructions}))}
    }
    match object.get("input").ok_or("Demande manquante.")? {
        serde_json::Value::String(text)=>messages.push(serde_json::json!({"role":"user","content":text})),
        serde_json::Value::Array(items)=>for value in items {
            let item=value.as_object().ok_or("Message Responses invalide.")?;
            if item.keys().any(|key|!matches!(key.as_str(),"type"|"role"|"content")) || item.get("type").is_some_and(|v|v.as_str()!=Some("message")){return Err("Élément Responses non pris en charge.".into())}
            let role=item.get("role").and_then(|v|v.as_str()).filter(|role|matches!(*role,"system"|"developer"|"user"|"assistant")).ok_or("Rôle Responses non pris en charge.")?;
            let content=item.get("content").ok_or("Contenu manquant.")?;
            let content=if let Some(text)=content.as_str(){serde_json::json!(text)}else{
                let parts=content.as_array().filter(|parts|!parts.is_empty()&&parts.len()<=128).ok_or("Contenu Responses non pris en charge.")?;
                let mut converted=Vec::new();
                for part in parts {
                    let part=part.as_object().ok_or("Fragment Responses invalide.")?;
                    let kind=part.get("type").and_then(|v|v.as_str());
                    let expected=if role=="assistant"{"output_text"}else{"input_text"};
                    if kind==Some(expected)&&part.keys().all(|key|matches!(key.as_str(),"type"|"text")){converted.push(serde_json::json!({"type":"text","text":part.get("text").and_then(|v|v.as_str()).ok_or("Texte Responses invalide.")?}));}
                    else if role=="user"&&kind==Some("input_image")&&part.keys().all(|key|matches!(key.as_str(),"type"|"image_url"|"detail")){let url=part.get("image_url").and_then(|v|v.as_str()).ok_or("Image Responses invalide.")?;let mut image=serde_json::json!({"url":url});if let Some(detail)=part.get("detail"){image["detail"]=detail.clone()}converted.push(serde_json::json!({"type":"image_url","image_url":image}));}
                    else{return Err("Fragment Responses non pris en charge.".into())}
                }
                if matches!(role,"system"|"developer"){let mut text=String::new();for part in &converted{text.push_str(part.get("text").and_then(|v|v.as_str()).ok_or("Instructions texte requises.")?)}serde_json::json!(text)}else{serde_json::Value::Array(converted)}
            };
            messages.push(serde_json::json!({"role":role,"content":content}));
        },
        _=>return Err("Entrée Responses invalide.".into()),
    }
    let mut chat=serde_json::json!({"model":model,"messages":messages});
    if let Some(stream)=object.get("stream"){chat["stream"]=stream.clone()}
    if let Some(limit)=object.get("max_output_tokens"){chat["max_tokens"]=limit.clone()}
    for key in ["temperature","top_p"]{if let Some(value)=object.get(key){chat[key]=value.clone()}}
    let (_,request,stream,limit)=simple_antigravity_chat(&chat)?;
    Ok((model,request,stream,limit))
}

fn antigravity_messages_effort(value:&serde_json::Value,model:&str,request:&mut serde_json::Value)->Result<(),String>{
    let Some(config)=value.get("output_config").filter(|v|!v.is_null()) else{return Ok(())};
    let config=config.as_object().ok_or("Configuration de sortie invalide.")?;
    if config.keys().any(|key|key!="effort"){return Err("Cette option de sortie Messages n'est pas prise en charge.".into())}
    let Some(effort)=config.get("effort").filter(|v|!v.is_null()) else{return Ok(())};
    let level=match effort.as_str(){Some("low")=>"LOW",Some("medium")=>"MEDIUM",Some("high")=>"HIGH",_=>return Err("Niveau de raisonnement Messages non pris en charge.".into())};
    if !model.starts_with("gemini-3") {return Err("Le réglage de raisonnement Messages est disponible expérimentalement uniquement pour les modèles Gemini 3.".into())}
    if !request.get("generationConfig").is_some_and(|v|v.is_object()){request["generationConfig"]=serde_json::json!({})}
    request["generationConfig"]["thinkingConfig"]=serde_json::json!({"thinkingLevel":level});Ok(())
}

fn simple_antigravity_messages(value:&serde_json::Value)->Result<(String,serde_json::Value,bool,u32),String>{
    let object=value.as_object().ok_or("Requête Messages invalide.")?;
    if let Some((key,_))=object.iter().find(|(key,value)|!matches!(key.as_str(),"model"|"messages"|"system"|"stream"|"max_tokens"|"temperature"|"top_p"|"stop_sequences"|"metadata"|"output_config"|"tools"|"tool_choice")&&!value.is_null()){return Err(format!("Option Messages non prise en charge pour Antigravity : {key}."))}
    if object.get("metadata").is_some_and(|v|!v.is_null()&&!v.is_object()){return Err("Métadonnées Messages invalides.".into())}
    if object.get("tools").is_some_and(|v|!v.is_null()&&v.as_array().is_none_or(|tools|!tools.is_empty())){return Err("Outils Messages invalides.".into())}
    if object.get("tool_choice").is_some_and(|v|!v.is_null()&&v.as_object().is_none_or(|o|o.len()!=1||o.get("type").and_then(|v|v.as_str())!=Some("none"))){return Err("Choix d’outil invalide.".into())}
    let model=object.get("model").and_then(|v|v.as_str()).filter(|s|!s.is_empty()).ok_or("Modèle manquant.")?;
    if object.get("max_tokens").is_none_or(|v|v.is_null()){return Err("max_tokens est requis.".into())}
    let mut messages=Vec::new();
    if let Some(system)=object.get("system").filter(|v|!v.is_null()){
        let text=if let Some(text)=system.as_str(){text.to_owned()}else{antigravity_messages_text(system)?};
        if !text.trim().is_empty(){messages.push(serde_json::json!({"role":"system","content":text}))}
    }
    let input=object.get("messages").and_then(|v|v.as_array()).ok_or("Messages manquants.")?;
    if input.len()>128{return Err("Historique trop volumineux.".into())}
    for message in input {
        let item=message.as_object().ok_or("Message invalide.")?;
        if item.keys().any(|key|!matches!(key.as_str(),"role"|"content")){return Err("Métadonnées Messages non prises en charge.".into())}
        let role=item.get("role").and_then(|v|v.as_str()).filter(|role|matches!(*role,"user"|"assistant")).ok_or("Rôle Messages invalide.")?;
        let content=item.get("content").ok_or("Contenu manquant.")?;
        let content=if content.is_string(){content.clone()}else{
            let parts=content.as_array().filter(|parts|!parts.is_empty()&&parts.len()<=128).ok_or("Contenu Messages invalide.")?;
            for part in parts{let item=part.as_object().ok_or("Bloc Messages invalide.")?;let valid=match item.get("type").and_then(|v|v.as_str()){
                Some("text")=>item.keys().all(|k|matches!(k.as_str(),"type"|"text"|"cache_control"))&&antigravity_cache_control_valid(item.get("cache_control")),
                Some("image") if role=="user"=>item.keys().all(|k|matches!(k.as_str(),"type"|"source"|"cache_control"))&&antigravity_cache_control_valid(item.get("cache_control"))&&item.get("source").and_then(|v|v.as_object()).is_some_and(|source|source.keys().all(|k|matches!(k.as_str(),"type"|"media_type"|"data"))&&source.get("type").and_then(|v|v.as_str())==Some("base64")),
                _=>false};if !valid{return Err("Bloc Messages non pris en charge.".into())}}
            crate::messages_adapter::text_or_parts(content)?
        };
        messages.push(serde_json::json!({"role":role,"content":content}));
    }
    let mut chat=serde_json::json!({"model":model,"messages":messages,"max_tokens":object["max_tokens"]});
    if let Some(stream)=object.get("stream"){chat["stream"]=stream.clone()}
    for key in ["temperature","top_p"]{if let Some(value)=object.get(key){chat[key]=value.clone()}}
    if let Some(value)=object.get("stop_sequences"){chat["stop"]=value.clone()}
    let (_,mut request,stream,limit)=simple_antigravity_chat(&chat)?;
    antigravity_messages_effort(value,model,&mut request)?;
    Ok((model.to_owned(),request,stream,limit))
}
fn antigravity_cache_control_valid(value:Option<&serde_json::Value>)->bool{
    match value{None|Some(serde_json::Value::Null)=>true,Some(value)=>value.as_object().is_some_and(|item|item.keys().all(|key|matches!(key.as_str(),"type"|"ttl"))&&item.get("type").and_then(|v|v.as_str())==Some("ephemeral")&&item.get("ttl").is_none_or(|v|matches!(v.as_str(),Some("5m"|"1h"))))}
}

fn antigravity_messages_text(value:&serde_json::Value)->Result<String,String>{
    let parts=value.as_array().filter(|parts|!parts.is_empty()&&parts.len()<=128).ok_or("Contenu Messages non pris en charge.")?;
    let mut result=String::new();
    for part in parts {
        let part=part.as_object().ok_or("Bloc Messages invalide.")?;
        if part.keys().any(|key|!matches!(key.as_str(),"type"|"text"|"cache_control"))||!antigravity_cache_control_valid(part.get("cache_control"))||part.get("type").and_then(|v|v.as_str())!=Some("text"){return Err(format!("Bloc Messages non pris en charge : type {}, champs {}.",part.get("type").and_then(|v|v.as_str()).unwrap_or("absent"),part.keys().cloned().collect::<Vec<_>>().join(", ")))}
        result.push_str(part.get("text").and_then(|v|v.as_str()).ok_or("Bloc texte invalide.")?);
        if result.len()>128_000{return Err("Historique trop volumineux.".into())}
    }
    Ok(result)
}

fn grok_headers(request:reqwest::RequestBuilder,model:Option<&str>)->Result<reqwest::RequestBuilder,String>{
    let mut request=request.header("x-xai-token-auth","xai-grok-cli");
    if let Some(model)=model {
        let value=reqwest::header::HeaderValue::from_str(model).map_err(|_|"Identifiant de modèle Grok invalide.")?;
        request=request.header("x-grok-model-override",value);
    }
    Ok(request)
}

async fn fetch_catalog(upstream: Arc<Upstream>) -> Result<Vec<serde_json::Value>, String> {
    if let Some((reference,id))=&upstream.antigravity {return Ok(crate::antigravity_login::gateway_catalog(reference,id).await?.into_iter().map(|id|serde_json::json!({"id":id,"object":"model"})).collect())}
    if let Some(reference)=&upstream.kiro {let token=crate::kiro_login::access(reference).await?;return crate::kiro_transport::catalog(token.as_str()).await}
    if is_cline(&upstream){return fetch_cline_catalog(&upstream.client,"https://api.cline.bot/api/v1/ai/cline/recommended-models".parse().unwrap()).await}
    tokio::time::timeout(Duration::from_secs(20), async move {
        let copilot=if let Some(reference)=&upstream.copilot {Some(crate::copilot_login::access(reference).await?)}else{None};
        let nous=if let Some(reference)=&upstream.nous {Some(crate::nous_login::access(reference).await?)}else{None};
        let mut url = match (&copilot,&nous) {(Some((_,base)),_)=>base,(_,Some((_,base)))=>base,_=>&upstream.base}.join("models").map_err(|_| "Adresse du fournisseur invalide.")?;
        if upstream.anthropic_native { url.query_pairs_mut().append_pair("limit","500"); }
        let mut request = upstream.client.get(url);
        if let Some(reference)=&upstream.chatgpt {let access=crate::openai_oauth::access(reference).await?;request=request.bearer_auth(access.as_str());}
        if let Some((access,_))=&copilot {request=request.bearer_auth(access.as_str()).header("editor-version","Nivra/0.1.0").header("editor-plugin-version","Nivra/0.1.0").header("copilot-integration-id","vscode-chat").header("user-agent","Nivra/0.1.0");}
        if let Some((access,_))=&nous {request=request.bearer_auth(access.as_str());}
        if let Some(reference)=&upstream.kimi {let access=crate::kimi_login::access(reference).await?;request=request.bearer_auth(access.as_str()).header("user-agent","KimiCLI/0.14.0").header("x-msh-platform","kimi_code_cli");}
        if let Some(reference)=&upstream.xai {let access=crate::xai_login::access(reference).await?;request=grok_headers(request.bearer_auth(access.as_str()),None)?;}
        if let Some(reference)=&upstream.qwen {let access=crate::qwen_login::access(reference).await?;request=request.bearer_auth(access.as_str()).header("x-dashscope-authtype","qwen-oauth");}
        if !upstream.key.is_empty() { request = if upstream.anthropic_native {request.header("x-api-key",&upstream.key)}else{request.bearer_auth(&upstream.key)}; }
        if upstream.anthropic_native { request=request.header("anthropic-version","2023-06-01"); }
        let mut result = request.send().await.map_err(|_| "Impossible de joindre le fournisseur.")?;
        if !result.status().is_success() { return Err(format!("Le fournisseur refuse la liste des modèles (HTTP {}).", result.status().as_u16())); }
        let mut data = Vec::new();
        while let Some(chunk) = result.chunk().await.map_err(|_| "Lecture des modèles interrompue.")? {
            if data.len() + chunk.len() > 2 * 1024 * 1024 { return Err("La liste des modèles est trop volumineuse.".into()); }
            data.extend_from_slice(&chunk);
        }
        let json: serde_json::Value = serde_json::from_slice(&data).map_err(|_| "Liste des modèles illisible.")?;
        if upstream.chatgpt.is_some(){return parse_chatgpt_catalog(&json)}
        let entries = json.get("data").and_then(|v| v.as_array()).ok_or("Ce fournisseur n'a pas renvoyé de liste compatible.")?;
        if entries.len()>500 || json.get("has_more").and_then(|value|value.as_bool())==Some(true) {return Err("Catalogue trop long : la pagination n'est pas encore prise en charge.".into());}
        Ok(entries.iter().filter(|entry| entry.get("id").and_then(|id| id.as_str()).is_some_and(|id| !id.is_empty() && id.len() <= 200 && !id.chars().any(char::is_control))).take(500).cloned().collect())
    }).await.map_err(|_| "La lecture des modèles a expiré.".to_string())?
}
async fn fetch_cline_catalog(client:&reqwest::Client,url:reqwest::Url)->Result<Vec<serde_json::Value>,String>{
    tokio::time::timeout(Duration::from_secs(15),async{
        let mut response=client.get(url).header("accept","application/json").send().await.map_err(|_|"Impossible de lire le catalogue Cline.")?;
        if !response.status().is_success(){return Err(format!("Lecture du catalogue Cline refusée (HTTP {}).",response.status().as_u16()))}let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|"Lecture du catalogue Cline interrompue.")?{if bytes.len().saturating_add(chunk.len())>2*1024*1024{return Err("Catalogue Cline trop volumineux.".into())}bytes.extend_from_slice(&chunk)}
        let data:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|"Catalogue Cline illisible.")?;let mut seen=std::collections::BTreeSet::new();let mut models=Vec::new();let mut found=false;
        for group in ["recommended","free","clinePass","clineCloud"]{if let Some(rows)=data.get(group){found=true;let rows=rows.as_array().filter(|rows|rows.len()<=500).ok_or("Catalogue Cline invalide.")?;for row in rows{let id=row.get("id").and_then(|v|v.as_str()).filter(|id|!id.is_empty()&&id.len()<=200&&!id.chars().any(|c|c.is_control()||c.is_whitespace())).ok_or("Identifiant de modèle Cline invalide.")?;if seen.insert(id.to_owned()){models.push(serde_json::json!({"id":id,"object":"model"}));if models.len()>500{return Err("Catalogue Cline trop volumineux.".into())}}}}}
        if !found{return Err("Catalogue Cline invalide.".into())}Ok(models)
    }).await.map_err(|_|"Lecture du catalogue Cline expirée.".to_string())?
}

fn parse_chatgpt_catalog(value:&serde_json::Value)->Result<Vec<serde_json::Value>,String>{
 let models=value.get("models").and_then(|v|v.as_array()).filter(|v|v.len()<=500).ok_or("Catalogue ChatGPT illisible.")?;
 Ok(models.iter().filter(|model|model.get("visibility").and_then(|v|v.as_str())==Some("list")).filter_map(|model|model.get("slug").and_then(|v|v.as_str()).filter(|id|!id.is_empty()&&id.len()<=200&&!id.chars().any(char::is_control)).map(|id|serde_json::json!({"id":id,"object":"model"}))).collect())
}
fn chatgpt_request(body:&Bytes)->Result<(),String>{let value:serde_json::Value=serde_json::from_slice(body).map_err(|_|"Requête ChatGPT invalide.")?;if value.get("stream").and_then(|v|v.as_bool())!=Some(true)||value.get("store").and_then(|v|v.as_bool())!=Some(false){return Err("ChatGPT nécessite stream=true et store=false.".into())}Ok(())}

fn safe_model_query(query:&str)->bool {
    let mut seen=std::collections::BTreeSet::new();
    for part in query.split('&') {
        let Some((key,value))=part.split_once('=') else{return false};
        if !seen.insert(key)||value.is_empty()||value.len()>200{return false}
        match key {
            "limit"=>if !value.bytes().all(|byte|byte.is_ascii_digit())||!value.parse::<u16>().is_ok_and(|number|(1..=1000).contains(&number)){return false},
            "after_id"|"before_id"=>if !value.bytes().all(|byte|byte.is_ascii_alphanumeric()||matches!(byte,b'-'|b'_'|b'.')){return false},
            _=>return false,
        }
    }
    !seen.is_empty() && !(seen.contains("after_id")&&seen.contains("before_id"))
}

async fn combined_catalog(router: &Router) -> Response<ResponseBody> {
    let providers: Vec<_> = match router.providers.read() {
        Ok(providers) => providers.iter().map(|(id, upstream)| (id.clone(), upstream.clone())).collect(),
        Err(_) => return response(StatusCode::INTERNAL_SERVER_ERROR, "Fournisseurs indisponibles."),
    };
    let total = providers.len();
    let results = futures_util::future::join_all(providers.into_iter().map(|(id, upstream)| async move { (id, fetch_catalog(upstream).await) })).await;
    let mut entries = Vec::new();
    let mut successes = 0;
    for (provider_id, result) in results {
        if let Ok(models) = result {
            successes += 1;
            for mut model in models {
                let Some(id) = model.get("id").and_then(|value| value.as_str()).map(str::to_owned) else { continue };
                if provider_id == router.default_id && !id.contains('/') { entries.push(model.clone()); }
                model["id"] = serde_json::Value::String(format!("{provider_id}/{id}"));
                entries.push(model);
            }
        }
    }
    if successes == 0 { return response(StatusCode::BAD_GATEWAY, "Aucun fournisseur n'a pu fournir sa liste de modèles."); }
    let body = serde_json::json!({"object": "list", "data": entries}).to_string();
    Response::builder().status(StatusCode::OK).header("content-type", "application/json").header("cache-control", "no-store").header("x-nivra-catalog-incomplete", if successes < total { "true" } else { "false" })
        .body(Full::new(Bytes::from(body)).map_err(|never| match never {}).boxed_unsync()).expect("static response")
}

fn response(status: StatusCode, message: &str) -> Response<ResponseBody> {
    let body = serde_json::json!({"error": {"message": message}}).to_string();
    Response::builder().status(status).header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)).map_err(|never| match never {}).boxed_unsync()).expect("static response")
}

fn messages_error(status: StatusCode, message: &str) -> Response<ResponseBody> {
    let kind=match status{StatusCode::UNAUTHORIZED=>"authentication_error",StatusCode::FORBIDDEN=>"permission_error",StatusCode::NOT_FOUND=>"not_found_error",StatusCode::PAYLOAD_TOO_LARGE=>"request_too_large",StatusCode::TOO_MANY_REQUESTS=>"rate_limit_error",StatusCode::SERVICE_UNAVAILABLE=>"overloaded_error",_ if status.is_client_error()=>"invalid_request_error",_=>"api_error"};
    let body=serde_json::json!({"type":"error","error":{"type":kind,"message":message}}).to_string();
    Response::builder().status(status).header("content-type","application/json").body(Full::new(Bytes::from(body)).map_err(|never|match never {}).boxed_unsync()).expect("static response")
}

pub(crate) fn upstream_url(raw: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(raw.trim()).map_err(|_| "Adresse du fournisseur invalide.")?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("Utilise une adresse HTTP(S) sans identifiants, paramètres ni fragment.".into());
    }
    if url.scheme() == "http" && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err("HTTP est réservé aux fournisseurs locaux ; utilise HTTPS pour les autres.".into());
    }
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url)
}

enum Translation { Responses(crate::responses_adapter::ToolMap), Messages(std::collections::BTreeSet<String>), ResponsesFromAnthropic(crate::responses_adapter::ToolMap), ChatFromAnthropic{include_usage:bool} }
struct AnthropicChatStream { bridge:crate::anthropic_upstream::StreamBridge, id:String, model:String, created:u64, failed:bool, include_usage:bool }
impl AnthropicChatStream {
    fn new(model:String,include_usage:bool)->Self{Self{bridge:crate::anthropic_upstream::StreamBridge::new(),id:format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()),model,created:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),failed:false,include_usage}}
    fn frame(&self,mut value:serde_json::Value)->Bytes {value["id"]=serde_json::json!(self.id);value["model"]=serde_json::json!(self.model);value["created"]=serde_json::json!(self.created);value["object"]=serde_json::json!("chat.completion.chunk");Bytes::from(format!("data: {value}\n\n"))}
    fn start(&self)->Vec<Bytes>{vec![self.frame(serde_json::json!({"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}))]}
    fn ingest(&mut self,bytes:&[u8])->Result<Vec<Bytes>,String>{
        let mut out=Vec::new();
        for part in self.bridge.ingest(bytes)?{
            if part.as_ref()==b"data: [DONE]\n\n"{out.push(part);continue}
            let text=std::str::from_utf8(&part).map_err(|_|"Flux traduit invalide.")?;
            let data=text.strip_prefix("data: ").ok_or("Flux traduit invalide.")?.trim_end();
            let value:serde_json::Value=serde_json::from_str(data).map_err(|_|"Flux traduit invalide.")?;
            if value.get("usage").is_some()&&!self.include_usage{continue}
            out.push(self.frame(value));
        }Ok(out)
    }
    fn fail(&mut self,message:&str)->Vec<Bytes>{self.failed=true;vec![Bytes::from(format!("data: {}\n\n",serde_json::json!({"error":{"message":message,"type":"api_error"}})))]}
}
enum StreamAdapter { Responses(crate::responses_adapter::StreamAdapter), Messages(crate::messages_adapter::StreamAdapter), AnthropicResponses{bridge:crate::anthropic_upstream::StreamBridge,responses:crate::responses_adapter::StreamAdapter}, AnthropicChat(AnthropicChatStream) }
impl StreamAdapter {
    fn start(&mut self)->Vec<Bytes>{match self{Self::Responses(adapter)=>adapter.start(),Self::Messages(adapter)=>adapter.start(),Self::AnthropicResponses{responses,..}=>responses.start(),Self::AnthropicChat(adapter)=>adapter.start()}}
    fn ingest(&mut self,bytes:&[u8])->Result<Vec<Bytes>,String>{match self{Self::Responses(adapter)=>adapter.ingest(bytes),Self::Messages(adapter)=>adapter.ingest(bytes),Self::AnthropicResponses{bridge,responses}=>{let mut result=Vec::new();for chunk in bridge.ingest(bytes)?{result.extend(responses.ingest(&chunk)?)}Ok(result)},Self::AnthropicChat(adapter)=>adapter.ingest(bytes)}}
    fn fail(&mut self,message:&str)->Vec<Bytes>{match self{Self::Responses(adapter)=>adapter.fail(message),Self::Messages(adapter)=>adapter.fail(message),Self::AnthropicResponses{responses,..}=>responses.fail(message),Self::AnthropicChat(adapter)=>adapter.fail(message)}}
    fn is_done(&self)->bool{match self{Self::Responses(adapter)=>adapter.is_done(),Self::Messages(adapter)=>adapter.is_done(),Self::AnthropicResponses{responses,..}=>responses.is_done(),Self::AnthropicChat(adapter)=>adapter.failed||adapter.bridge.is_done()}}
}
struct ConvertedStream { response: reqwest::Response, adapter: StreamAdapter, queue: VecDeque<Bytes> }

fn anthropic_chat_request(value:&serde_json::Value)->Result<(serde_json::Value,bool),String>{
    let object=value.as_object().ok_or("Requête Chat invalide.")?;
    const SUPPORTED:&[&str]=&["model","messages","max_tokens","max_completion_tokens","stream","stream_options","temperature","top_p","stop","reasoning_effort","response_format","tools","tool_choice","parallel_tool_calls","n"];
    if object.iter().any(|(key,value)|!SUPPORTED.contains(&key.as_str())&&!value.is_null()){return Err("Paramètre Chat non pris en charge pour Anthropic.".into())}
    if object.get("n").is_some_and(|value|!value.is_null()&&value.as_u64()!=Some(1)){return Err("Anthropic ne prend en charge qu'une réponse par requête.".into())}
    if object.contains_key("max_tokens")&&object.contains_key("max_completion_tokens"){return Err("Indique une seule limite de sortie.".into())}
    if object.get("stream").is_some_and(|value|!value.is_boolean()){return Err("Option de flux invalide.".into())}
    for key in ["max_tokens","max_completion_tokens"] {if object.get(key).is_some_and(|value|value.as_u64().is_none_or(|n|n==0)){return Err("Limite de sortie invalide.".into())}}
    let include_usage=match object.get("stream_options"){
        None|Some(serde_json::Value::Null)=>false,
        Some(value)=>{let options=value.as_object().ok_or("Options de flux invalides.")?;if options.keys().any(|key|key!="include_usage"){return Err("Option de flux non prise en charge.".into())}options.get("include_usage").map(|value|value.as_bool().ok_or("Option de flux invalide.")).transpose()?.unwrap_or(false)}
    };
    let mut normalized=value.clone();
    if let Some(max)=normalized.as_object_mut().and_then(|object|object.remove("max_completion_tokens")){normalized["max_tokens"]=max}
    Ok((crate::anthropic_upstream::request(&normalized)?,include_usage))
}

async fn translated_response(mut received: reqwest::Response, model: String, streaming: bool, translation:Translation) -> Response<ResponseBody> {
    let is_messages=matches!(translation,Translation::Messages(_));
    let failure=|message:&str|if is_messages{messages_error(StatusCode::BAD_GATEWAY,message)}else{response(StatusCode::BAD_GATEWAY,message)};
    if !streaming {
        let mut body = Vec::new();
        loop {
            match received.chunk().await {
                Ok(Some(chunk)) if body.len()+chunk.len() <= MAX_REQUEST => body.extend_from_slice(&chunk),
                Ok(Some(_)) => return failure("Réponse du fournisseur trop volumineuse."),
                Ok(None) => break,
                Err(_) => return failure("Lecture du fournisseur interrompue."),
            }
        }
        let result = serde_json::from_slice::<serde_json::Value>(&body).map_err(|_| "Réponse du fournisseur illisible.".to_string()).and_then(|value| match &translation {Translation::Responses(mapping)=>crate::responses_adapter::non_stream(&value,&model,mapping),Translation::Messages(allowed)=>crate::messages_adapter::non_stream(&value,&model,allowed),Translation::ResponsesFromAnthropic(mapping)=>crate::anthropic_upstream::non_stream(&value).and_then(|chat|crate::responses_adapter::non_stream(&chat,&model,mapping)),Translation::ChatFromAnthropic{..}=>crate::anthropic_upstream::non_stream(&value).map(|mut chat|{chat["id"]=serde_json::json!(format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()));chat["object"]=serde_json::json!("chat.completion");chat["created"]=serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());chat["model"]=serde_json::json!(model);chat})});
        return match result {
            Ok(value) => Response::builder().status(StatusCode::OK).header("content-type", "application/json").header("cache-control", "no-store").body(Full::new(Bytes::from(value.to_string())).map_err(|never| match never {}).boxed_unsync()).expect("static response"),
            Err(message) => failure(&message),
        };
    }
    if !received.headers().get("content-type").and_then(|value| value.to_str().ok()).is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream")) {
        return failure("Le fournisseur n'a pas renvoyé un flux compatible.");
    }
    let mut adapter = match translation {Translation::Responses(mapping)=>StreamAdapter::Responses(crate::responses_adapter::StreamAdapter::new(model,mapping)),Translation::Messages(allowed)=>StreamAdapter::Messages(crate::messages_adapter::StreamAdapter::new(model,allowed)),Translation::ResponsesFromAnthropic(mapping)=>StreamAdapter::AnthropicResponses{bridge:crate::anthropic_upstream::StreamBridge::new(),responses:crate::responses_adapter::StreamAdapter::new(model,mapping)},Translation::ChatFromAnthropic{include_usage}=>StreamAdapter::AnthropicChat(AnthropicChatStream::new(model,include_usage))};
    let queue = VecDeque::from(adapter.start());
    let state = ConvertedStream { response: received, adapter, queue };
    let chunks = stream::unfold(state, |mut state| async move {
        loop {
            if let Some(chunk) = state.queue.pop_front() { return Some((Ok::<_, std::io::Error>(Frame::data(chunk)), state)); }
            if state.adapter.is_done() { return None; }
            match state.response.chunk().await {
                Ok(Some(chunk)) => match state.adapter.ingest(&chunk) {
                    Ok(events) => state.queue.extend(events),
                    Err(message) => state.queue.extend(state.adapter.fail(&message)),
                },
                Ok(None) => state.queue.extend(state.adapter.fail("Flux du fournisseur terminé prématurément.")),
                Err(_) => state.queue.extend(state.adapter.fail("Lecture du fournisseur interrompue.")),
            }
        }
    });
    Response::builder().status(StatusCode::OK).header("content-type", "text/event-stream").header("cache-control", "no-store").body(StreamBody::new(chunks).boxed_unsync()).expect("static response")
}

struct GoogleChatStream { id:String, model:String, created:u64, pending:Vec<u8>, data:Vec<String>, done:bool, text_bytes:usize }
impl GoogleChatStream {
    fn new(model:String)->Self{Self{id:format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()),model,created:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),pending:Vec::new(),data:Vec::new(),done:false,text_bytes:0}}
    fn frame(&self,delta:serde_json::Value,finish:Option<&str>)->Bytes{let event=serde_json::json!({"id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]});Bytes::from(format!("data: {event}\n\n"))}
    fn ingest(&mut self,chunk:&[u8])->Result<Vec<Bytes>,String>{
        if self.done{return Err("Données après la fin du flux Antigravity.".into())}
        self.pending.extend_from_slice(chunk);if self.pending.len()>2*1024*1024{return Err("Événement Antigravity trop volumineux.".into())}
        let mut out=Vec::new();
        while let Some(end)=self.pending.iter().position(|b|*b==b'\n'){
            let mut line:Vec<_>=self.pending.drain(..=end).collect();line.pop();if line.last()==Some(&b'\r'){line.pop();}
            let line=std::str::from_utf8(&line).map_err(|_|"Flux Antigravity illisible.")?;
            if line.is_empty(){
                if self.data.is_empty(){continue}let data=std::mem::take(&mut self.data).join("\n");
                let value:serde_json::Value=serde_json::from_str(&data).map_err(|_|"Événement Antigravity invalide.")?;
                if value.get("error").is_some(){return Err("Antigravity a interrompu la réponse.".into())}
                let metadata=value.pointer("/response/usageMetadata");
                if let Some((input,output))=metadata.and_then(crate::antigravity_login::token_usage){
                    let event=serde_json::json!({"choices":[],"usage":{"prompt_tokens":input,"completion_tokens":output,"total_tokens":input.saturating_add(output)}});
                    out.push(Bytes::from(format!("data: {event}\n\n")));
                }
                let candidates=value.pointer("/response/candidates").and_then(|v|v.as_array());
                let Some(candidates)=candidates else {if metadata.is_some(){continue}else{return Err("Événement Antigravity sans résultat.".into())}};
                if candidates.len()!=1{return Err("Nombre de résultats Antigravity invalide.".into())}
                let candidate=&candidates[0];
                if let Some(parts)=candidate.pointer("/content/parts").and_then(|v|v.as_array()){
                    for part in parts {if part.get("thought").and_then(|v|v.as_bool())==Some(true){continue}
                        let text=part.get("text").and_then(|v|v.as_str()).ok_or("Fragment Antigravity non textuel non pris en charge.")?;
                        self.text_bytes=self.text_bytes.checked_add(text.len()).ok_or("Réponse trop volumineuse.")?;if self.text_bytes>1024*1024{return Err("Réponse Antigravity trop volumineuse.".into())}
                        if !text.is_empty(){out.push(self.frame(serde_json::json!({"content":text}),None))}
                    }
                }
                if let Some(reason)=candidate.get("finishReason").and_then(|v|v.as_str()){
                    let finish=match reason{"STOP"=>"stop","MAX_TOKENS"=>"length",_=>return Err("Réponse Antigravity bloquée ou interrompue.".into())};
                    out.push(self.frame(serde_json::json!({}),Some(finish)));out.push(Bytes::from_static(b"data: [DONE]\n\n"));self.done=true;break
                }
            }else if let Some(value)=line.strip_prefix("data:"){
                if self.data.len()>=8192||value.len()+self.data.iter().map(String::len).sum::<usize>()>2*1024*1024{return Err("Événement Antigravity trop volumineux.".into())}self.data.push(value.strip_prefix(' ').unwrap_or(value).to_owned());
            }
        }
        Ok(out)
    }
}
fn antigravity_stream_response(received:reqwest::Response,model:String,responses:bool)->Response<ResponseBody>{
    let adapter=GoogleChatStream::new(model.clone());
    let mut translate=responses.then(||crate::responses_adapter::StreamAdapter::new(model,crate::responses_adapter::ToolMap::new()));
    let first=if let Some(translate)=translate.as_mut(){translate.start()}else{vec![adapter.frame(serde_json::json!({"role":"assistant"}),None)]};
    let state=(received,adapter,VecDeque::from(first),false,translate);
    let chunks=stream::unfold(state,|mut state|async move{
        loop{
            if let Some(chunk)=state.2.pop_front(){return Some((Ok::<_,std::io::Error>(Frame::data(chunk)),state))}
            if state.1.done||state.3{return None}
            let incoming=match tokio::time::timeout(Duration::from_secs(60),state.0.chunk()).await{
                Ok(Ok(Some(chunk)))=>state.1.ingest(&chunk),
                _=>Err("Flux Antigravity interrompu ou expiré.".into()),
            };
            match incoming.and_then(|chunks|{
                if let Some(translate)=state.4.as_mut(){let mut events=Vec::new();for chunk in chunks{events.extend(translate.ingest(&chunk)?)}Ok(events)}else{Ok(chunks)}
            }){
                Ok(chunks)=>state.2.extend(chunks),
                Err(message)=>{state.3=true;if let Some(translate)=state.4.as_mut(){state.2.extend(translate.fail(&message))}else{return Some((Err(std::io::Error::other(message)),state))}}
            }
        }
    });
    Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store").body(StreamBody::new(chunks).boxed_unsync()).expect("static response")
}

fn chatgpt_text_stream(received:reqwest::Response,model:String)->Response<ResponseBody>{
    let adapter=crate::chatgpt_adapter::TextStream::new(model);let first=adapter.start();let state=(received,adapter,VecDeque::from([first]),false);
    let chunks=stream::unfold(state,|mut state|async move{loop{
        if let Some(chunk)=state.2.pop_front(){return Some((Ok::<_,std::io::Error>(Frame::data(chunk)),state))}
        if state.1.done()||state.3{return None}
        let incoming=match tokio::time::timeout(Duration::from_secs(120),state.0.chunk()).await{Ok(Ok(Some(chunk)))=>state.1.ingest(&chunk),_=>Err("Flux ChatGPT interrompu ou expiré.".into())};
        match incoming{Ok(chunks)=>state.2.extend(chunks),Err(message)=>{state.3=true;return Some((Err(std::io::Error::other(message)),state))}}
    }});
    Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store").body(StreamBody::new(chunks).boxed_unsync()).expect("static response")
}

async fn forward(request:Request<Incoming>,router:Arc<Router>)->Result<Response<ResponseBody>,Infallible>{
    forward_with_antigravity(request,router,crate::antigravity_login::generate_for_account,true).await
}

async fn forward_with_antigravity<F,Fut>(request:Request<Incoming>,router:Arc<Router>,send:F,real_stream:bool)->Result<Response<ResponseBody>,Infallible>
where F:Fn(String,String,String,serde_json::Value,u32)->Fut, Fut:std::future::Future<Output=Result<crate::antigravity_login::GeneratedAnswer,String>> {
    let is_messages=matches!(request.uri().path(),"/v1/messages"|"/v1/messages/count_tokens");
    let is_count=request.uri().path()=="/v1/messages/count_tokens";
    let version=request.headers().get("anthropic-version").cloned();
    let beta=request.headers().get("anthropic-beta").cloned();
    let workspace=request.headers().get("anthropic-workspace-id").cloned();
    let query=request.uri().query().map(str::to_owned);
    let failure=|status,message:&str|if is_messages{messages_error(status,message)}else{response(status,message)};
    let bearer=request.headers().get(hyper::header::AUTHORIZATION).and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer "));
    let api_key=request.headers().get("x-api-key").and_then(|v|v.to_str().ok());
    let authorized=match (bearer,api_key) {(Some(a),Some(b))=>a==router.token && b==router.token,(Some(a),None)|(None,Some(a))=>a==router.token,_=>false};
    if !authorized { return Ok(failure(StatusCode::UNAUTHORIZED, "Clé de passerelle invalide.")); }
    let (method, suffix) = match (request.method(), request.uri().path()) {
        (&Method::GET, "/v1/models") => (reqwest::Method::GET, "models"),
        (&Method::POST, "/v1/chat/completions") => (reqwest::Method::POST, "chat/completions"),
        (&Method::POST, "/v1/responses") => (reqwest::Method::POST, "responses"),
        (&Method::POST, "/v1/messages") => (reqwest::Method::POST, "messages"),
        (&Method::POST, "/v1/messages/count_tokens") => (reqwest::Method::POST, "messages/count_tokens"),
        _ => return Ok(response(StatusCode::NOT_FOUND, "Route non prise en charge.")),
    };
    let multiple = router.providers.read().map(|providers| providers.len() > 1).unwrap_or(false);
    if request.uri().query().is_some_and(|query|!(is_messages&&query=="beta=true")&&!(method==reqwest::Method::GET&&suffix=="models"&&!multiple&&safe_model_query(query))) { return Ok(failure(StatusCode::BAD_REQUEST, "Paramètres d’URL non pris en charge.")); }
    if method == reqwest::Method::GET && query.is_none() { return Ok(combined_catalog(&router).await); }
    let body = match Limited::new(request.into_body(), MAX_REQUEST).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return Ok(failure(StatusCode::PAYLOAD_TOO_LARGE, "Requête trop volumineuse ou interrompue.")),
    };
    let client_model = serde_json::from_slice::<serde_json::Value>(&body).ok().and_then(|value| value.get("model").and_then(|value| value.as_str()).map(str::to_owned)).unwrap_or_default();
    let (upstream, mut body) = if method == reqwest::Method::POST {
        match select_provider(&router, &body) { Ok(value) => value, Err(message) => return Ok(failure(StatusCode::BAD_REQUEST, &message)) }
    } else {
        let default = router.providers.read().ok().and_then(|providers| providers.get(&router.default_id).cloned());
        match default { Some(value) => (value, body), None => return Ok(response(StatusCode::BAD_GATEWAY, "Fournisseur par défaut indisponible.")) }
    };
    if let Some(reference)=&upstream.kiro {
        if !matches!(suffix,"chat/completions"|"responses"|"messages"){return Ok(failure(StatusCode::NOT_IMPLEMENTED,"Cette route n'est pas encore disponible pour Kiro."))}
        let value:serde_json::Value=match serde_json::from_slice(&body){Ok(value)=>value,Err(_)=>return Ok(failure(StatusCode::BAD_REQUEST,"Requête Kiro invalide."))};
        let streaming=match value.get("stream"){None=>false,Some(v)=>match v.as_bool(){Some(v)=>v,None=>return Ok(failure(StatusCode::BAD_REQUEST,"Option de flux Kiro invalide."))}};
        let mut message_tools=None;
        let (mut chat,mapping)=if suffix=="responses"{match crate::responses_adapter::request(&value){Ok((chat,mapping))=>(chat,Some(mapping)),Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))}}else if suffix=="messages"{
            let Some(object)=value.as_object()else{return Ok(failure(StatusCode::BAD_REQUEST,"Requête Messages invalide."))};if object.iter().any(|(key,v)|!matches!(key.as_str(),"model"|"messages"|"system"|"max_tokens"|"stream"|"tools"|"tool_choice"|"thinking")&&!v.is_null()){return Ok(failure(StatusCode::BAD_REQUEST,"Option Messages non prise en charge pour Kiro."))}
            if let Some(messages)=object.get("messages").and_then(|v|v.as_array()){for message in messages{if message.as_object().is_none_or(|m|m.keys().any(|k|!matches!(k.as_str(),"role"|"content"))){return Ok(failure(StatusCode::BAD_REQUEST,"Option de message non prise en charge pour Kiro."))}if let Some(parts)=message.get("content").and_then(|v|v.as_array()){for part in parts{let Some(item)=part.as_object()else{return Ok(failure(StatusCode::BAD_REQUEST,"Bloc Messages invalide."))};let valid=match item.get("type").and_then(|v|v.as_str()){
                Some("text")=>item.keys().all(|k|matches!(k.as_str(),"type"|"text")),
                Some("image")=>item.keys().all(|k|matches!(k.as_str(),"type"|"source"))&&item.get("source").and_then(|v|v.as_object()).is_some_and(|s|s.get("type").and_then(|v|v.as_str())==Some("base64")&&s.keys().all(|k|matches!(k.as_str(),"type"|"media_type"|"data"))),
                Some("tool_use")=>item.keys().all(|k|matches!(k.as_str(),"type"|"id"|"name"|"input")),
                Some("tool_result")=>item.keys().all(|k|matches!(k.as_str(),"type"|"tool_use_id"|"content"|"is_error"))&&item.get("is_error").is_none_or(|v|v.is_boolean()),_=>false};if !valid{return Ok(failure(StatusCode::BAD_REQUEST,"Bloc Messages non pris en charge pour Kiro."))}}}}}
            match crate::messages_adapter::request(&value){Ok((chat,allowed))=>{message_tools=Some(allowed);(chat,None)},Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))}
        }else{(value,None)};
        if let Some(messages)=chat.get_mut("messages").and_then(|v|v.as_array_mut()){for message in messages{if let Some(parts)=message.get("content").and_then(|v|v.as_array()){if parts.iter().all(|part|part.get("type").and_then(|v|v.as_str())==Some("text")){let mut text=String::new();for part in parts{let Some(value)=part.get("text").and_then(|v|v.as_str())else{return Ok(failure(StatusCode::BAD_REQUEST,"Texte Kiro invalide."))};text.push_str(value)}message["content"]=serde_json::json!(text)}}}}
        let Some(model)=chat.get("model").and_then(|v|v.as_str()).map(str::to_owned)else{return Ok(failure(StatusCode::BAD_REQUEST,"Modèle Kiro manquant."))};
        let mut request=match crate::kiro_request::request(&chat,&model){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))};
        if let Err(message)=crate::kiro_thinking::restore(&router.token,reference,&model,&mut request){return Ok(failure(StatusCode::BAD_REQUEST,&message))}
        let allowed=chat.get("tools").and_then(|v|v.as_array()).map(|tools|tools.iter().filter_map(|tool|tool.pointer("/function/name").and_then(|v|v.as_str()).map(str::to_owned)).collect()).unwrap_or_default();
        let token=match crate::kiro_login::access(reference).await{Ok(token)=>token,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};
        if crate::kiro_request::needs_schema(&chat){
            #[cfg(not(test))]let catalog=crate::kiro_transport::catalog(token.as_str()).await;
            #[cfg(test)]let catalog=if reference=="synthetic-kiro-test"&&upstream.base.scheme()=="http"{crate::kiro_transport::catalog_at(&upstream.client,upstream.base.join("ListAvailableModels").unwrap(),token.as_str()).await}else{crate::kiro_transport::catalog(token.as_str()).await};
            let catalog=match catalog{Ok(catalog)=>catalog,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};let schema=catalog.iter().find(|entry|entry.get("id").and_then(|v|v.as_str())==Some(model.as_str())).and_then(|entry|entry.get("additionalModelRequestFieldsSchema"));if let Err(message)=crate::kiro_request::apply_options(&mut request,&chat,schema){return Ok(failure(StatusCode::BAD_REQUEST,&message))}}
        #[cfg(not(test))]let generated=crate::kiro_transport::generate(token.as_str(),&request,client_model.clone(),allowed).await;
        #[cfg(test)]let generated=if reference=="synthetic-kiro-test"&&upstream.base.scheme()=="http"{crate::kiro_transport::generate_at(&upstream.client,upstream.base.join("generateAssistantResponse").unwrap(),token.as_str(),&request,client_model.clone(),allowed).await}else{crate::kiro_transport::generate(token.as_str(),&request,client_model.clone(),allowed).await};
        let mut result=match generated{Ok(result)=>result,Err(error)=>{let status=StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY);let mut response=failure(status,&error.message);if let Some(seconds)=error.retry_after{if let Ok(header)=hyper::header::HeaderValue::from_str(&seconds.to_string()){response.headers_mut().insert(hyper::header::RETRY_AFTER,header);}}return Ok(response)}};
        if let Err(message)=crate::kiro_thinking::store(&router.token,reference,&model,&request,&mut result){return Ok(failure(StatusCode::BAD_GATEWAY,&message))}
        let inferred_finish=result.get("nivra_finish_reason_inferred").and_then(|v|v.as_bool())==Some(true);
        let estimated_usage=message_tools.is_some()&&result.get("usage").is_none();if estimated_usage{result["usage"]=match crate::kiro_response::estimate_text_usage(&chat,&result){Ok(usage)=>usage,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};}
        if chat.get("parallel_tool_calls").and_then(|v|v.as_bool())==Some(false)&&result.pointer("/choices/0/message/tool_calls").and_then(|v|v.as_array()).is_some_and(|calls|calls.len()>1){return Ok(failure(StatusCode::BAD_GATEWAY,"Kiro a renvoyé plusieurs outils alors qu'un seul était autorisé."))}
        if streaming{
            let input=match crate::kiro_response::buffered_stream(&result){Ok(input)=>input,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};
            let output=if let Some(mapping)=mapping{let mut adapter=crate::responses_adapter::StreamAdapter::new(client_model,mapping);let mut chunks=adapter.start();match adapter.ingest(&input){Ok(next)=>chunks.extend(next),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}chunks.into_iter().flat_map(|c|c.to_vec()).collect()}else if let Some(allowed)=message_tools{let mut adapter=crate::messages_adapter::StreamAdapter::new(client_model,allowed);let mut chunks=adapter.start_with_input_tokens(result["usage"]["prompt_tokens"].as_u64().unwrap());match adapter.ingest(&input){Ok(next)=>chunks.extend(next),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}chunks.into_iter().flat_map(|c|c.to_vec()).collect()}else{input};
            let mut response=Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store");if inferred_finish{response=response.header("x-nivra-kiro-finish","inferred-from-metering")};if estimated_usage{response=response.header("x-nivra-usage-source","estimated")};return Ok(response.body(Full::new(Bytes::from(output)).map_err(|never|match never{}).boxed_unsync()).unwrap());
        }
        if let Some(mapping)=mapping{result=match crate::responses_adapter::non_stream(&result,&client_model,&mapping){Ok(result)=>result,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}}
        if let Some(allowed)=message_tools{result=match crate::messages_adapter::non_stream(&result,&client_model,&allowed){Ok(result)=>result,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}}
        let mut response=Response::builder().status(StatusCode::OK).header("content-type","application/json").header("cache-control","no-store");if inferred_finish{response=response.header("x-nivra-kiro-finish","inferred-from-metering")};if estimated_usage{response=response.header("x-nivra-usage-source","estimated")};return Ok(response.body(Full::new(Bytes::from(result.to_string())).map_err(|never|match never{}).boxed_unsync()).unwrap());
    }
    let mut chatgpt_chat=None;
    if upstream.chatgpt.is_some(){
        if suffix=="chat/completions"||suffix=="messages"{let value=match serde_json::from_slice::<serde_json::Value>(&body){Ok(value)=>value,Err(_)=>return Ok(failure(StatusCode::BAD_REQUEST,"Requête invalide."))};let (chat,allowed)=if suffix=="messages"{let Some(object)=value.as_object()else{return Ok(failure(StatusCode::BAD_REQUEST,"Requête Messages invalide."))};if let Some((key,_))=object.iter().find(|(key,v)|!matches!(key.as_str(),"model"|"messages"|"system"|"max_tokens"|"stream"|"tools"|"tool_choice"|"thinking")&&!v.is_null()){return Ok(failure(StatusCode::BAD_REQUEST,&format!("Option Messages non prise en charge : {key}.")))}match crate::messages_adapter::request(&value){Ok(converted)=>converted,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))}}else{let allowed=value.get("tools").and_then(|v|v.as_array()).map(|tools|tools.iter().filter_map(|tool|tool.pointer("/function/name").and_then(|v|v.as_str()).map(str::to_owned)).collect::<std::collections::BTreeSet<_>>()).unwrap_or_default();(value,allowed)};match crate::chatgpt_adapter::request(&chat){Ok((converted,stream))=>{body=Bytes::from(converted.to_string());chatgpt_chat=Some((stream,allowed,suffix=="messages"));},Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))}}
        else if suffix!="responses"{return Ok(failure(StatusCode::NOT_IMPLEMENTED,"Ce compte ChatGPT accepte Responses, Chat et Messages."))}
        if let Err(message)=chatgpt_request(&body){return Ok(failure(StatusCode::BAD_REQUEST,&message))}
    }
    if let Some((reference,id))=&upstream.antigravity {
        if !matches!(suffix,"chat/completions"|"responses"|"messages") {return Ok(failure(StatusCode::NOT_IMPLEMENTED,"Antigravity prend uniquement en charge les demandes texte simples pour le moment."))}
        let value=match serde_json::from_slice::<serde_json::Value>(&body){Ok(value)=>value,Err(_)=>return Ok(failure(StatusCode::BAD_REQUEST,"Requête invalide."))};
        let tool_mode=(suffix=="chat/completions"&&(value.get("tools").and_then(|v|v.as_array()).is_some_and(|v|!v.is_empty())||value.get("messages").and_then(|v|v.as_array()).is_some_and(|items|items.iter().any(|item|item.get("tool_calls").is_some_and(|v|!v.is_null())||item.get("role").and_then(|v|v.as_str())==Some("tool")))))||(suffix=="responses"&&(value.get("tools").and_then(|v|v.as_array()).is_some_and(|v|!v.is_empty())||value.get("input").and_then(|v|v.as_array()).is_some_and(|items|items.iter().any(|item|matches!(item.get("type").and_then(|v|v.as_str()),Some("function_call"|"function_call_output"))))))||(suffix=="messages"&&(value.get("tools").and_then(|v|v.as_array()).is_some_and(|v|!v.is_empty())||value.get("messages").and_then(|v|v.as_array()).is_some_and(|items|items.iter().any(|item|item.get("content").and_then(|v|v.as_array()).is_some_and(|parts|parts.iter().any(|part|matches!(part.get("type").and_then(|v|v.as_str()),Some("tool_use"|"tool_result"))))))));
        let mut response_tool_map=None;
        let mut message_tools=None;
        let parsed=if tool_mode{
            let converted=(||->Result<_,String>{
                let streaming=match value.get("stream"){None=>false,Some(v)=>v.as_bool().ok_or("Option de flux invalide.")?};
                let model=value.get("model").and_then(|v|v.as_str()).filter(|s|!s.is_empty()).ok_or("Modèle manquant.")?;
                let chat=if suffix=="responses"{
                    let object=value.as_object().ok_or("Requête Responses invalide.")?;
                    if let Some((key,_))=object.iter().find(|(key,v)|!matches!(key.as_str(),"model"|"input"|"instructions"|"tools"|"tool_choice"|"stream"|"store"|"max_output_tokens"|"temperature"|"top_p"|"client_metadata"|"include"|"parallel_tool_calls"|"prompt_cache_key")&&!v.is_null()){return Err(format!("Option Responses non prise en charge pour Antigravity : {key}."))}
                    if object.get("store").is_some_and(|v|!v.is_null()&&v.as_bool()!=Some(false)){return Err("Stockage non pris en charge.".into())}
                    if object.get("client_metadata").is_some_and(|v|!v.is_null()&&!v.is_object()){return Err("Métadonnées client invalides.".into())}
                    if object.get("prompt_cache_key").is_some_and(|v|!v.is_null()&&v.as_str().is_none_or(|s|s.len()>1024)){return Err("Clé de cache invalide.".into())}
                    if object.get("parallel_tool_calls").is_some_and(|v|!v.is_null()&&!v.is_boolean()){return Err("Option d’appels parallèles invalide.".into())}
                    if let Some(items)=object.get("input").and_then(|v|v.as_array()){for item in items{if let Some(parts)=item.get("content").and_then(|v|v.as_array()){for part in parts{if part.get("type").and_then(|v|v.as_str())==Some("input_image")&&part.as_object().is_none_or(|o|o.keys().any(|k|!matches!(k.as_str(),"type"|"image_url"|"detail"))){return Err("Option d’image Responses non prise en charge.".into())}}}}}
                    let (mut chat,mapping)=crate::responses_adapter::request(&value)?;
                    chat.as_object_mut().unwrap().remove("parallel_tool_calls");
                    if let Some(messages)=chat.get_mut("messages").and_then(|v|v.as_array_mut()){for message in messages{if let Some(parts)=message.get("content").and_then(|v|v.as_array()){if parts.iter().all(|part|part.get("type").and_then(|v|v.as_str())==Some("text")){let mut text=String::new();for part in parts{text.push_str(part.get("text").and_then(|v|v.as_str()).ok_or("Texte invalide.")?)}message["content"]=serde_json::json!(text)}}}}
                    if let Some(messages)=chat.get_mut("messages").and_then(|v|v.as_array_mut()){
                        let mut i=1;while i<messages.len(){
                            if messages[i].get("tool_calls").is_some_and(|v|v.is_array())&&messages[i-1].get("role").and_then(|v|v.as_str())==Some("assistant")&&messages[i-1].get("tool_calls").is_none(){
                                let content=messages[i-1].get("content").cloned().ok_or("Message assistant invalide.")?;
                                messages[i]["content"]=content;messages.remove(i-1);
                            }else{i+=1}
                        }
                    }
                    if let Some(tools)=chat.get_mut("tools").and_then(|v|v.as_array_mut()){for tool in tools{if let Some(function)=tool.get_mut("function").and_then(|v|v.as_object_mut()){if function.get("strict")==Some(&serde_json::json!(false)){function.remove("strict");}}}}
                    response_tool_map=Some(mapping);chat
                }else if suffix=="messages"{
                    let object=value.as_object().ok_or("Requête Messages invalide.")?;
                    if object.iter().any(|(key,v)|!matches!(key.as_str(),"model"|"messages"|"system"|"tools"|"tool_choice"|"stream"|"max_tokens"|"temperature"|"top_p"|"stop_sequences"|"metadata"|"output_config")&&!v.is_null()){return Err("Option Messages non prise en charge pour Antigravity.".into())}
                    if object.get("metadata").is_some_and(|v|!v.is_null()&&!v.is_object()){return Err("Métadonnées Messages invalides.".into())}
                    if let Some(messages)=object.get("messages").and_then(|v|v.as_array()){for message in messages{
                        if message.as_object().is_none_or(|item|item.keys().any(|key|!matches!(key.as_str(),"role"|"content"))){return Err("Métadonnées Messages non prises en charge.".into())}
                        if let Some(parts)=message.get("content").and_then(|v|v.as_array()){for part in parts{
                            let item=part.as_object().ok_or("Bloc Messages invalide.")?;
                            let valid=match item.get("type").and_then(|v|v.as_str()){
                                Some("text")=>item.keys().all(|k|matches!(k.as_str(),"type"|"text"|"cache_control"))&&antigravity_cache_control_valid(item.get("cache_control")),
                                Some("image") if message.get("role").and_then(|v|v.as_str())==Some("user")=>item.keys().all(|k|matches!(k.as_str(),"type"|"source"|"cache_control"))&&antigravity_cache_control_valid(item.get("cache_control"))&&item.get("source").and_then(|v|v.as_object()).is_some_and(|source|source.keys().all(|k|matches!(k.as_str(),"type"|"media_type"|"data"))&&source.get("type").and_then(|v|v.as_str())==Some("base64")),
                                Some("tool_use")=>item.keys().all(|k|matches!(k.as_str(),"type"|"id"|"name"|"input")),
                                Some("tool_result")=>item.keys().all(|k|matches!(k.as_str(),"type"|"tool_use_id"|"content"|"is_error"|"cache_control"))&&antigravity_cache_control_valid(item.get("cache_control"))&&item.get("content").is_some_and(|v|v.is_string()||v.as_array().is_some_and(|parts|parts.iter().all(|p|p.as_object().is_some_and(|o|o.keys().all(|k|matches!(k.as_str(),"type"|"text"))&&o.get("type").and_then(|v|v.as_str())==Some("text")&&o.get("text").is_some_and(|v|v.is_string())))))&&item.get("is_error").is_none_or(|v|v.is_boolean()),
                                _=>false};
                            if !valid{return Err(format!("Bloc Messages non pris en charge : type {}, champs {}.",item.get("type").and_then(|v|v.as_str()).unwrap_or("absent"),item.keys().cloned().collect::<Vec<_>>().join(", ")))}
                        }}
                    }}
                    if let Some(system)=object.get("system").filter(|v|!v.is_null()&&!v.is_string()){antigravity_messages_text(system)?;}
                    if let Some(tools)=object.get("tools").and_then(|v|v.as_array()){for tool in tools{if tool.as_object().is_none_or(|o|o.keys().any(|k|!matches!(k.as_str(),"name"|"type"|"description"|"input_schema"))){return Err("Option d’outil Messages non prise en charge.".into())}}}
                    if let Some(choice)=object.get("tool_choice").filter(|v|!v.is_null()){
                        let choice=choice.as_object().ok_or("Choix d’outil Messages invalide.")?;
                        let valid=match choice.get("type").and_then(|v|v.as_str()){
                            Some("auto"|"any"|"none")=>choice.len()==1,
                            Some("tool")=>choice.len()==2&&choice.get("name").is_some_and(|v|v.is_string()),
                            _=>false};
                        if !valid{return Err("Choix d’outil Messages non pris en charge.".into())}
                    }
                    let (mut chat,allowed)=crate::messages_adapter::request(&value)?;
                    if let Some(messages)=chat.get_mut("messages").and_then(|v|v.as_array_mut()){for message in messages{if let Some(parts)=message.get("content").and_then(|v|v.as_array()){if parts.iter().all(|part|part.get("type").and_then(|v|v.as_str())==Some("text")){let mut text=String::new();for part in parts{text.push_str(part.get("text").and_then(|v|v.as_str()).ok_or("Texte invalide.")?)}message["content"]=serde_json::json!(text)}}}}
                    message_tools=Some(allowed);chat
                }else{value.clone()};
                let old=chat.get("max_tokens").filter(|v|!v.is_null());let new=chat.get("max_completion_tokens").filter(|v|!v.is_null());if old.is_some()&&new.is_some(){return Err("Indique une seule limite de sortie.".into())}
                let limit=antigravity_output_limit(old.or(new))?;
                let mut request=crate::antigravity_tools::prepare(&router.token,reference,id,model,&chat)?;
                if suffix=="messages"{antigravity_messages_effort(&value,model,&mut request)?}
                Ok((model.to_owned(),request,streaming,limit))
            })();converted
        }else if suffix=="messages"{simple_antigravity_messages(&value)}else if suffix=="responses"{simple_antigravity_responses(&value).map(|(model,request,stream,limit)|(model.to_owned(),request,stream,limit))}else{simple_antigravity_chat(&value).map(|(model,request,stream,limit)|(model.to_owned(),request,stream,limit))};
        let (model,prompt,streaming,max_tokens)=match parsed{Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))};
        if real_stream && streaming && !tool_mode && matches!(suffix,"chat/completions"|"responses") {
            let received=match crate::antigravity_login::stream_for_account(reference.clone(),id.clone(),model.to_owned(),prompt,max_tokens).await{Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};
            return Ok(antigravity_stream_response(received,client_model,suffix=="responses"));
        }
        let tool_request=tool_mode.then(||prompt.clone());
        let answer=match send(reference.clone(),id.clone(),model.to_owned(),prompt,max_tokens).await{Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};
        let usage=answer.input_tokens.zip(answer.output_tokens).map(|(input,output)|serde_json::json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":input.saturating_add(output)}));
        if answer.reply.is_none()&&tool_request.as_ref().and_then(|v|v.pointer("/toolConfig/functionCallingConfig/mode")).and_then(|v|v.as_str())==Some("ANY"){return Ok(failure(StatusCode::BAD_GATEWAY,"Antigravity n'a pas appelé l’outil requis."))}
        if let Some(reply)=&answer.reply {
            if suffix=="responses"&&value.get("parallel_tool_calls").and_then(|v|v.as_bool())==Some(false)&&reply.chat.pointer("/choices/0/message/tool_calls").and_then(|v|v.as_array()).is_some_and(|v|v.len()>1){return Ok(failure(StatusCode::BAD_GATEWAY,"Antigravity a renvoyé plusieurs outils alors qu'un seul était autorisé."))}
            let Some(request)=tool_request.as_ref() else{return Ok(failure(StatusCode::BAD_GATEWAY,"Antigravity a renvoyé un outil non demandé."))};
            if let Err(message)=crate::antigravity_tools::store(&router.token,reference,id,&model,request,reply){return Ok(failure(StatusCode::BAD_GATEWAY,&message))}
            let mut result=reply.chat.clone();result["id"]=serde_json::json!(format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()));result["object"]=serde_json::json!("chat.completion");result["created"]=serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());result["model"]=serde_json::json!(client_model);
            if let Some(usage)=usage{result["usage"]=usage}
            if streaming {
                let message=&result["choices"][0]["message"];let mut events=Vec::new();
                if let Some(text)=message.get("content").and_then(|v|v.as_str()){events.push(serde_json::json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]}));}
                let calls=message.get("tool_calls").and_then(|v|v.as_array()).ok_or(());
                let calls=match calls{Ok(calls)=>calls,Err(())=>return Ok(failure(StatusCode::BAD_GATEWAY,"Appel d’outil invalide."))};
                let mut converted=Vec::new();for (index,call) in calls.iter().enumerate(){converted.push(serde_json::json!({"index":index,"id":call["id"],"type":"function","function":call["function"]}));}
                events.push(serde_json::json!({"choices":[{"index":0,"delta":{"tool_calls":converted},"finish_reason":null}]}));
                events.push(serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":result.get("usage")}));
                let input=events.iter().map(|event|format!("data: {event}\n\n")).collect::<String>()+"data: [DONE]\n\n";
                let body=if let Some(mapping)=response_tool_map.as_ref(){
                    let mut adapter=crate::responses_adapter::StreamAdapter::new(client_model.clone(),mapping.clone());let mut output=adapter.start();match adapter.ingest(input.as_bytes()){Ok(events)=>output.extend(events),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}output.into_iter().flat_map(|chunk|chunk.to_vec()).collect::<Vec<_>>()
                }else if let Some(allowed)=message_tools.as_ref(){
                    if result.get("usage").is_none(){return Ok(failure(StatusCode::BAD_GATEWAY,"Antigravity n'a pas renvoyé le nombre de tokens nécessaire à Messages."))}
                    let mut adapter=crate::messages_adapter::StreamAdapter::new(client_model.clone(),allowed.clone());let mut output=adapter.start_with_input_tokens(result["usage"]["prompt_tokens"].as_u64().unwrap());match adapter.ingest(input.as_bytes()){Ok(events)=>output.extend(events),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}output.into_iter().flat_map(|chunk|chunk.to_vec()).collect::<Vec<_>>()
                }else{
                    let id=result["id"].as_str().unwrap_or("");let created=result["created"].as_u64().unwrap_or(0);
                    let mut chunks=vec![serde_json::json!({"id":id,"object":"chat.completion.chunk","created":created,"model":client_model,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]})];
                    for mut event in events{event["id"]=serde_json::json!(id);event["object"]=serde_json::json!("chat.completion.chunk");event["created"]=serde_json::json!(created);event["model"]=serde_json::json!(client_model);chunks.push(event)}
                    (chunks.iter().map(|event|format!("data: {event}\n\n")).collect::<String>()+"data: [DONE]\n\n").into_bytes()
                };
                return Ok(Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store").body(Full::new(Bytes::from(body)).map_err(|never|match never{}).boxed_unsync()).unwrap());
            }
            if let Some(mapping)=response_tool_map.as_ref(){result=match crate::responses_adapter::non_stream(&result,&client_model,mapping){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};}
            if let Some(allowed)=message_tools.as_ref(){if result.get("usage").is_none(){return Ok(failure(StatusCode::BAD_GATEWAY,"Antigravity n'a pas renvoyé le nombre de tokens nécessaire à Messages."))}result=match crate::messages_adapter::non_stream(&result,&client_model,allowed){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};}
            return Ok(Response::builder().status(StatusCode::OK).header("content-type","application/json").header("cache-control","no-store").body(Full::new(Bytes::from(result.to_string())).map_err(|never|match never{}).boxed_unsync()).unwrap());
        }

        let finish=if answer.truncated{"length"}else{"stop"};
        let message_finish=if answer.truncated{"max_tokens"}else{"end_turn"};
        let answer=answer.text;
        if suffix=="messages"{
            let Some(usage)=usage else{return Ok(failure(StatusCode::BAD_GATEWAY,"Antigravity n'a pas renvoyé le nombre de tokens nécessaire à Messages."))};
            let input=usage["prompt_tokens"].as_u64().unwrap();let output=usage["completion_tokens"].as_u64().unwrap();
            let id=format!("msg_{}",uuid::Uuid::new_v4().simple());
            if streaming {
                let events=[
                    ("message_start",serde_json::json!({"type":"message_start","message":{"id":id,"type":"message","role":"assistant","content":[],"model":client_model,"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":input,"output_tokens":0}}})),
                    ("content_block_start",serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
                    ("content_block_delta",serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":answer}})),
                    ("content_block_stop",serde_json::json!({"type":"content_block_stop","index":0})),
                    ("message_delta",serde_json::json!({"type":"message_delta","delta":{"stop_reason":message_finish,"stop_sequence":null},"usage":{"output_tokens":output}})),
                    ("message_stop",serde_json::json!({"type":"message_stop"}))
                ];
                let body=events.iter().map(|(name,event)|format!("event: {name}\ndata: {event}\n\n")).collect::<String>();
                return Ok(Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store").body(Full::new(Bytes::from(body)).map_err(|never|match never{}).boxed_unsync()).unwrap());
            }
            let result=serde_json::json!({"id":id,"type":"message","role":"assistant","model":client_model,"content":[{"type":"text","text":answer}],"stop_reason":message_finish,"stop_sequence":null,"usage":{"input_tokens":input,"output_tokens":output}});
            return Ok(Response::builder().status(StatusCode::OK).header("content-type","application/json").header("cache-control","no-store").body(Full::new(Bytes::from(result.to_string())).map_err(|never|match never{}).boxed_unsync()).unwrap());
        }
        if suffix=="responses"{
            let mapping=crate::responses_adapter::ToolMap::new();
            if streaming {
                let mut adapter=crate::responses_adapter::StreamAdapter::new(client_model,mapping);let mut chunks=adapter.start();
                let content=serde_json::json!({"choices":[{"index":0,"delta":{"content":answer},"finish_reason":null}]});
                let stop=serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":finish}],"usage":usage});
                let events=format!("data: {content}\n\ndata: {stop}\n\ndata: [DONE]\n\n");
                match adapter.ingest(events.as_bytes()){Ok(events)=>chunks.extend(events),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}
                let body=chunks.into_iter().flat_map(|chunk|chunk.to_vec()).collect::<Vec<_>>();
                return Ok(Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store").body(Full::new(Bytes::from(body)).map_err(|never|match never{}).boxed_unsync()).unwrap());
            }
            let chat=serde_json::json!({"choices":[{"message":{"role":"assistant","content":answer},"finish_reason":finish}],"usage":usage});
            let result=match crate::responses_adapter::non_stream(&chat,&client_model,&mapping){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};
            return Ok(Response::builder().status(StatusCode::OK).header("content-type","application/json").header("cache-control","no-store").body(Full::new(Bytes::from(result.to_string())).map_err(|never|match never{}).boxed_unsync()).unwrap());
        }
        let id=format!("chatcmpl-{}",uuid::Uuid::new_v4().simple());let created=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        if streaming {
            let role=serde_json::json!({"id":id,"object":"chat.completion.chunk","created":created,"model":client_model,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]});
            let content=serde_json::json!({"id":id,"object":"chat.completion.chunk","created":created,"model":client_model,"choices":[{"index":0,"delta":{"content":answer},"finish_reason":null}]});
            let stop=serde_json::json!({"id":id,"object":"chat.completion.chunk","created":created,"model":client_model,"choices":[{"index":0,"delta":{},"finish_reason":finish}]});
            let body=format!("data: {role}\n\ndata: {content}\n\ndata: {stop}\n\ndata: [DONE]\n\n");
            return Ok(Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store").body(Full::new(Bytes::from(body)).map_err(|never|match never{}).boxed_unsync()).unwrap());
        }
        let mut result=serde_json::json!({"id":id,"object":"chat.completion","created":created,"model":client_model,"choices":[{"index":0,"message":{"role":"assistant","content":answer},"finish_reason":finish}]});
        if let Some(usage)=usage{result["usage"]=usage;}
        return Ok(Response::builder().status(StatusCode::OK).header("content-type","application/json").header("cache-control","no-store").body(Full::new(Bytes::from(result.to_string())).map_err(|never|match never{}).boxed_unsync()).unwrap());
    }
    if suffix=="models" && query.is_some() && !upstream.anthropic_native {return Ok(failure(StatusCode::BAD_REQUEST,"Pagination indisponible pour ce fournisseur."));}
    if is_count && !upstream.anthropic_native {
        return Ok(failure(StatusCode::NOT_IMPLEMENTED,"Le comptage de tokens n'est pas disponible pour ce fournisseur."));
    }
    let model_name=serde_json::from_slice::<serde_json::Value>(&body).ok().and_then(|v|v.get("model").and_then(|v|v.as_str()).map(str::to_owned));
    let anthropic_wire=anthropic_wire_for(&upstream,model_name.as_deref());
    let native_responses=suffix=="responses"&&model_name.as_deref().is_some_and(|model|(upstream.copilot.is_some()&&copilot_responses_model(model))||(is_opencode_go(&upstream)&&opencode_go_responses_model(model)));
    let translate_responses = suffix == "responses" && upstream.chat_only && !native_responses;
    let mut streaming = false;
    let mut translation = None;
    let target = if chatgpt_chat.is_some(){"responses"}else if anthropic_wire && suffix=="responses" {
        let value=match serde_json::from_slice::<serde_json::Value>(&body){Ok(value)=>value,Err(_)=>return Ok(failure(StatusCode::BAD_REQUEST,"Requête invalide."))};
        streaming=value.get("stream").and_then(|value|value.as_bool()).unwrap_or(false);
        let (chat,mapping)=match crate::responses_adapter::request(&value){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))};
        let converted=match crate::anthropic_upstream::request(&chat){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))};
        body=Bytes::from(converted.to_string());translation=Some(Translation::ResponsesFromAnthropic(mapping));"messages"
    } else if anthropic_wire && suffix=="chat/completions" {
        let value=match serde_json::from_slice::<serde_json::Value>(&body){Ok(value)=>value,Err(_)=>return Ok(failure(StatusCode::BAD_REQUEST,"Requête invalide."))};
        streaming=value.get("stream").and_then(|value|value.as_bool()).unwrap_or(false);
        let (converted,include_usage)=match anthropic_chat_request(&value){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))};
        body=Bytes::from(converted.to_string());translation=Some(Translation::ChatFromAnthropic{include_usage});"messages"
    } else if translate_responses || (is_messages && !anthropic_wire) {
        let value = match serde_json::from_slice::<serde_json::Value>(&body) { Ok(value) => value, Err(_) => return Ok(failure(StatusCode::BAD_REQUEST, "Requête invalide.")) };
        streaming = value.get("stream").and_then(|value| value.as_bool()).unwrap_or(false);
        let converted = if is_messages {
            match crate::messages_adapter::request(&value){Ok((value,allowed))=>{translation=Some(Translation::Messages(allowed));value},Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))}
        }else{
            match crate::responses_adapter::request(&value){Ok((value,tools))=>{translation=Some(Translation::Responses(tools));value},Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))}
        };
        body = Bytes::from(converted.to_string());
        "chat/completions"
    } else { suffix };
    let mut grok_buffered=false;
    if upstream.xai.is_some()&&target=="chat/completions"&&method==reqwest::Method::POST {
        let mut value=match serde_json::from_slice::<serde_json::Value>(&body){Ok(value)=>value,Err(_)=>return Ok(failure(StatusCode::BAD_REQUEST,"Requête Grok invalide."))};
        match value.get("stream"){None|Some(serde_json::Value::Bool(false))=>{value["stream"]=serde_json::json!(true);body=Bytes::from(value.to_string());grok_buffered=true},Some(serde_json::Value::Bool(true))=>{},_=>return Ok(failure(StatusCode::BAD_REQUEST,"Option de flux Grok invalide."))}
    }
    let copilot=if let Some(reference)=&upstream.copilot {match crate::copilot_login::access(reference).await{Ok(value)=>Some(value),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}}else{None};
    let nous=if let Some(reference)=&upstream.nous {match crate::nous_login::access(reference).await{Ok(value)=>Some(value),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}}else{None};
    let mut url = match match (&copilot,&nous){(Some((_,base)),_)=>base,(_,Some((_,base)))=>base,_=>&upstream.base}.join(target) { Ok(url) => url, Err(_) => return Ok(failure(StatusCode::BAD_GATEWAY, "Adresse du fournisseur invalide.")) };
    if anthropic_wire && query.as_deref()==Some("beta=true") { url.set_query(Some("beta=true")); }
    if upstream.anthropic_native && suffix=="models" {url.set_query(query.as_deref());}
    let mut outbound = upstream.client.request(method.clone(), url);
    if let Some(reference)=&upstream.chatgpt {let access=match crate::openai_oauth::access(reference).await{Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};outbound=outbound.bearer_auth(access.as_str()).header("accept","text/event-stream");}
    if let Some((access,_))=&copilot {outbound=outbound.bearer_auth(access.as_str()).header("editor-version","Nivra/0.1.0").header("editor-plugin-version","Nivra/0.1.0").header("copilot-integration-id","vscode-chat").header("user-agent","Nivra/0.1.0");}
    if let Some((access,_))=&nous {outbound=outbound.bearer_auth(access.as_str());}
    if let Some(reference)=&upstream.kimi {let access=match crate::kimi_login::access(reference).await{Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};outbound=outbound.bearer_auth(access.as_str()).header("user-agent","KimiCLI/0.14.0").header("x-msh-platform","kimi_code_cli");}
    if let Some(reference)=&upstream.xai {let access=match crate::xai_login::access(reference).await{Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};outbound=match grok_headers(outbound.bearer_auth(access.as_str()),model_name.as_deref()){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))};}
    if let Some(reference)=&upstream.qwen {let access=match crate::qwen_login::access(reference).await{Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};outbound=outbound.bearer_auth(access.as_str()).header("x-dashscope-authtype","qwen-oauth");}
    if !upstream.key.is_empty() { outbound = if anthropic_wire {outbound.header("x-api-key",&upstream.key)}else{outbound.bearer_auth(&upstream.key)}; }
    if anthropic_wire {
        outbound=outbound.header("anthropic-version",version.unwrap_or_else(||hyper::header::HeaderValue::from_static("2023-06-01")));
        if let Some(beta)=beta {outbound=outbound.header("anthropic-beta",beta)}
        if let Some(workspace)=workspace {outbound=outbound.header("anthropic-workspace-id",workspace)}
    }
    if method == reqwest::Method::POST {if is_cline(&upstream)&&target=="chat/completions"{body=match cline_chat_body(&body){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_REQUEST,&message))};outbound=outbound.header("x-response-format","openai");}outbound = outbound.header("content-type", "application/json").body(body); }
    let mut received = match outbound.send().await {
        Ok(value) => value,
        Err(_) => return Ok(failure(StatusCode::BAD_GATEWAY, "Impossible de joindre le fournisseur.")),
    };
    let status = received.status();
    if grok_buffered&&status.is_success(){
        if !received.headers().get("content-type").and_then(|v|v.to_str().ok()).is_some_and(|v|v.to_ascii_lowercase().starts_with("text/event-stream")){return Ok(failure(StatusCode::BAD_GATEWAY,"Grok n’a pas renvoyé de flux compatible."))}
        let mut bytes=Vec::new();loop{let next=match tokio::time::timeout(Duration::from_secs(120),received.chunk()).await{Ok(Ok(value))=>value,_=>return Ok(failure(StatusCode::BAD_GATEWAY,"Le flux Grok a été interrompu."))};let Some(chunk)=next else{break};if bytes.len()+chunk.len()>MAX_REQUEST{return Ok(failure(StatusCode::BAD_GATEWAY,"Réponse Grok trop volumineuse."))}bytes.extend_from_slice(&chunk)}
        let result=crate::grok_stream::complete(&bytes,&client_model).and_then(|chat|match &translation{None=>Ok(chat),Some(Translation::Responses(mapping))=>crate::responses_adapter::non_stream(&chat,&client_model,mapping),Some(Translation::Messages(allowed))=>crate::messages_adapter::non_stream(&chat,&client_model,allowed),_=>Err("Conversion Grok indisponible.".into())});
        return Ok(match result{Ok(value)=>Response::builder().status(StatusCode::OK).header("content-type","application/json").header("cache-control","no-store").body(Full::new(Bytes::from(value.to_string())).map_err(|never|match never{}).boxed_unsync()).unwrap(),Err(message)=>if is_messages{messages_error(StatusCode::BAD_GATEWAY,&message)}else{failure(StatusCode::BAD_GATEWAY,&message)}})
    }
    if let Some((stream,allowed,messages))=chatgpt_chat{
        if !status.is_success(){return Ok(failure(status,"ChatGPT a refusé la requête."))}
        if stream&&!messages&&allowed.is_empty(){return Ok(chatgpt_text_stream(received,client_model))}
        let mut bytes=Vec::new();loop{let next=match tokio::time::timeout(Duration::from_secs(120),received.chunk()).await{Ok(Ok(value))=>value,_=>return Ok(failure(StatusCode::BAD_GATEWAY,"Le flux ChatGPT a été interrompu."))};let Some(chunk)=next else{break};if bytes.len()+chunk.len()>16*1024*1024{return Ok(failure(StatusCode::BAD_GATEWAY,"Réponse ChatGPT trop volumineuse."))}bytes.extend_from_slice(&chunk)}
        let text=match std::str::from_utf8(&bytes){Ok(value)=>value,Err(_)=>return Ok(failure(StatusCode::BAD_GATEWAY,"Flux ChatGPT illisible."))};let result=match crate::chatgpt_adapter::completed(text,&client_model,&allowed){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};
        if messages{if result.get("usage").is_none(){return Ok(failure(StatusCode::BAD_GATEWAY,"ChatGPT n’a pas fourni l’usage de la réponse."))}let result=match crate::messages_adapter::non_stream(&result,&client_model,&allowed){Ok(value)=>value,Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))};let (content_type,output)=if stream{match crate::chatgpt_adapter::messages_stream(&result){Ok(events)=>("text/event-stream",events),Err(message)=>return Ok(failure(StatusCode::BAD_GATEWAY,&message))}}else{("application/json",result.to_string())};return Ok(Response::builder().status(StatusCode::OK).header("content-type",content_type).header("cache-control","no-store").body(Full::new(Bytes::from(output)).map_err(|never|match never{}).boxed_unsync()).unwrap())}
        if stream{let message=&result["choices"][0]["message"];let mut delta=serde_json::json!({"role":"assistant","content":message["content"]});if let Some(calls)=message.get("tool_calls").and_then(|v|v.as_array()){delta["tool_calls"]=serde_json::Value::Array(calls.iter().enumerate().map(|(index,call)|{let mut call=call.clone();call["index"]=serde_json::json!(index);call}).collect());}let chunk=serde_json::json!({"id":result["id"],"object":"chat.completion.chunk","created":result["created"],"model":result["model"],"choices":[{"index":0,"delta":delta,"finish_reason":null}]});let finish=serde_json::json!({"id":result["id"],"object":"chat.completion.chunk","created":result["created"],"model":result["model"],"choices":[{"index":0,"delta":{},"finish_reason":result["choices"][0]["finish_reason"]}]});let body=format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n");return Ok(Response::builder().status(StatusCode::OK).header("content-type","text/event-stream").header("cache-control","no-store").body(Full::new(Bytes::from(body)).map_err(|never|match never{}).boxed_unsync()).unwrap())}
        return Ok(Response::builder().status(StatusCode::OK).header("content-type","application/json").header("cache-control","no-store").body(Full::new(Bytes::from(result.to_string())).map_err(|never|match never{}).boxed_unsync()).unwrap())
    }
    if let Some(translation)=translation {
        if status.is_success(){return Ok(translated_response(received,client_model,streaming,translation).await)}
        if is_messages{return Ok(messages_error(status,"Le fournisseur a refusé la requête."))}
        if anthropic_wire{return Ok(response(status,"Le fournisseur Anthropic a refusé la requête."))}
    }
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = received.headers().get("content-type") { builder = builder.header("content-type", content_type); }
    builder = builder.header("cache-control", "no-store");
    let chunks = stream::unfold(Some(received), |received| async move {
        let mut received = received?;
        match received.chunk().await {
            Ok(Some(chunk)) => Some((Ok(Frame::data(chunk)), Some(received))),
            Ok(None) => None,
            Err(_) => Some((Err(std::io::Error::other("Lecture du fournisseur interrompue")), None)),
        }
    });
    Ok(builder.body(StreamBody::new(chunks).boxed_unsync()).expect("valid upstream response"))
}

#[tauri::command]
pub async fn gateway_start(manager: tauri::State<'_, GatewayManager>, provider_id: String, base_url: String, api_key: String, credential_ref: Option<String>, chat_only: bool, anthropic_native:bool, port: u16) -> Result<GatewayStatus, String> {
    if !valid_id(&provider_id) { return Err("Identifiant invalide : utilise des lettres minuscules, chiffres, tirets ou underscores.".into()); }
    if credential_ref.is_some() && !api_key.is_empty() { return Err("Choisis une clé saisie ou une clé enregistrée.".into()); }
    let key = match credential_ref { Some(reference) => crate::vault::load_key(&reference,&base_url,chat_only,anthropic_native)?, None => api_key };
    let upstream = create_upstream_with_mode(&base_url, key, chat_only,anthropic_native)?;
    start_gateway(&manager,provider_id,upstream,port)
}

fn start_gateway(manager:&GatewayManager,provider_id:String,upstream:Arc<Upstream>,port:u16)->Result<GatewayStatus,String>{
    if !valid_id(&provider_id){return Err("Identifiant de routage invalide.".into())}
    if port == 0 { return Err("Choisis un port entre 1 et 65535.".into()); }
    let mut guard = manager.0.lock().map_err(|_| "Passerelle indisponible.")?;
    if guard.is_some() { return Err("La passerelle est déjà active. Arrête-la avant de modifier sa configuration.".into()); }
    let listener = std::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)).map_err(|_| "Port indisponible sur cet appareil.".to_string())?;
    listener.set_nonblocking(true).map_err(|_| "Impossible de démarrer la passerelle.")?;
    let listener = TcpListener::from_std(listener).map_err(|_| "Impossible de démarrer la passerelle.")?;
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let status = GatewayStatus { address: format!("http://127.0.0.1:{port}/v1"), token: token.clone(), base_url: upstream.base.to_string() };
    let router = Arc::new(Router { token, default_id: provider_id.clone(), providers: RwLock::new(BTreeMap::from([(provider_id, upstream)])) });
    let (shutdown, mut stopped) = watch::channel(false);
    let slots = Arc::new(Semaphore::new(32));
    let task_router = router.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = stopped.changed() => break,
                result = listener.accept() => match result {
                    Ok((socket, _)) => {
                        let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
                        let router = task_router.clone();
                        let mut connection_stop = stopped.clone();
                        tauri::async_runtime::spawn(async move {
                            let _permit = permit;
                            let service = service_fn(move |request| forward(request, router.clone()));
                            tokio::select! {
                                _ = connection_stop.changed() => {},
                                _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service) => {},
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    });
    *guard = Some(Running { shutdown, status: status.clone(), router });
    Ok(status)
}

#[tauri::command]
pub fn gateway_stop(manager: tauri::State<'_, GatewayManager>) -> Result<(), String> {
    let running = manager.0.lock().map_err(|_| "Passerelle indisponible.")?.take();
    if let Some(running) = running { let _ = running.shutdown.send(true); }
    Ok(())
}

#[tauri::command]
pub fn gateway_status(manager: tauri::State<'_, GatewayManager>) -> Result<Option<GatewayStatus>, String> {
    let mut guard = manager.0.lock().map_err(|_| "Passerelle indisponible.")?;
    if guard.as_ref().is_some_and(|running| running.shutdown.is_closed()) { *guard = None; }
    Ok(guard.as_ref().map(|running| running.status.clone()))
}

fn current_router(manager: &GatewayManager) -> Result<Arc<Router>, String> {
    let guard = manager.0.lock().map_err(|_| "Passerelle indisponible.")?;
    Ok(guard.as_ref().filter(|running| !running.shutdown.is_closed()).ok_or("La passerelle n’est pas active.")?.router.clone())
}

fn valid_launch_model(model:&str)->bool {
    !model.is_empty() && model.len()<=200 && !model.starts_with('-') && !model.chars().any(|character|character.is_whitespace()||character.is_control())
}

fn launch_provider(router:&Router, model:&str)->Result<Arc<Upstream>,String>{
    if !valid_launch_model(model){return Err("Identifiant de modèle invalide.".into())}
    let providers=router.providers.read().map_err(|_|"Fournisseurs indisponibles.")?;
    let id=if let Some((prefix,name))=model.split_once('/'){
        if name.is_empty(){return Err("Indique un modèle après le fournisseur.".into())}
        prefix
    }else{&router.default_id};
    providers.get(id).cloned().ok_or_else(||"Fournisseur inconnu : utilise l’identifiant d’un fournisseur actif suivi de /modèle.".into())
}

#[cfg(windows)]
const CODEX_LAUNCH_SCRIPT:&str = r#"
$provider = $env:NIVRA_CODEX_PROVIDER
$settings = @(
  ('model_provider="' + $provider + '"'),
  ('model_providers.' + $provider + '.name="Nivra"'),
  ('model_providers.' + $provider + '.base_url="' + $env:NIVRA_GATEWAY_URL + '"'),
  ('model_providers.' + $provider + '.env_key="NIVRA_GATEWAY_KEY"'),
  ('model_providers.' + $provider + '.wire_api="responses"'),
  ('model_providers.' + $provider + '.requires_openai_auth=false'),
  ('model_providers.' + $provider + '.supports_websockets=false')
)
if ($env:NIVRA_CODEX_NO_WEB -eq '1') { $settings += 'web_search="disabled"' }
$arguments = @('-C', $env:NIVRA_CODEX_WORKSPACE, '-m', $env:NIVRA_CODEX_MODEL)
foreach ($setting in $settings) { $arguments += '-c'; $arguments += $setting }
& $env:NIVRA_CODEX_PATH @arguments
"#;

#[cfg(windows)]
pub(crate) fn codex_executable()->Result<std::path::PathBuf,String>{
    let installed=std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).map(|path|path.join("Programs/OpenAI/Codex/bin/codex.exe"));
    if let Some(path)=installed.filter(|path|path.is_file()){return Ok(path)}
    if let Some(paths)=std::env::var_os("PATH"){
        if let Some(path)=std::env::split_paths(&paths).map(|folder|folder.join("codex.exe")).find(|path|path.is_file()){return Ok(path)}
    }
    Err("Codex CLI est introuvable. Installe-le puis réessaie.".into())
}

#[cfg(windows)]
fn codex_launch_command(executable:&std::path::Path, workspace:&std::path::Path, model:&str, address:&str, token:&str, disable_web:bool)->Result<std::process::Command,String>{
    use std::os::windows::process::CommandExt;
    let powershell=std::env::var_os("SystemRoot").map(std::path::PathBuf::from).map(|path|path.join("System32/WindowsPowerShell/v1.0/powershell.exe")).filter(|path|path.is_file()).ok_or("PowerShell est introuvable.")?;
    let provider=format!("nivra_{}",uuid::Uuid::new_v4().simple());
    let mut no_proxy=std::env::var("NO_PROXY").unwrap_or_default();
    if !no_proxy.is_empty(){no_proxy.push(',')}no_proxy.push_str("127.0.0.1,localhost");
    let mut command=std::process::Command::new(powershell);
    command.args(["-NoLogo","-NoExit","-Command",CODEX_LAUNCH_SCRIPT])
        .env("NIVRA_CODEX_PATH",executable).env("NIVRA_CODEX_WORKSPACE",workspace).env("NIVRA_CODEX_MODEL",model)
        .env("NIVRA_CODEX_PROVIDER",provider).env("NIVRA_GATEWAY_URL",address).env("NIVRA_GATEWAY_KEY",token)
        .env("NIVRA_CODEX_NO_WEB",if disable_web{"1"}else{"0"}).env("NO_PROXY",&no_proxy).env("no_proxy",no_proxy)
        .current_dir(workspace).creation_flags(0x00000010);
    Ok(command)
}

#[cfg(windows)]
pub(crate) fn claude_executable()->Result<std::path::PathBuf,String>{
    let candidates=[std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).map(|path|path.join(".local/bin/claude.exe")),std::env::var_os("APPDATA").map(std::path::PathBuf::from).map(|path|path.join("npm/node_modules/@anthropic-ai/claude-code/bin/claude.exe"))];
    if let Some(path)=candidates.into_iter().flatten().find(|path|path.is_file()){return Ok(path)}
    if let Some(paths)=std::env::var_os("PATH"){
        if let Some(path)=std::env::split_paths(&paths).map(|folder|folder.join("claude.exe")).find(|path|path.is_file()){return Ok(path)}
    }
    Err("Claude Code est introuvable. Installe-le puis réessaie.".into())
}

#[cfg(windows)]
fn claude_launch_command(executable:&std::path::Path,workspace:&std::path::Path,model:&str,address:&str,token:&str)->Result<std::process::Command,String>{
    use std::os::windows::process::CommandExt;
    let parsed=reqwest::Url::parse(address).map_err(|_|"Adresse locale invalide.")?;
    let origin=parsed.origin().ascii_serialization();
    let mut no_proxy=std::env::var("NO_PROXY").unwrap_or_default();
    if !no_proxy.is_empty(){no_proxy.push(',')}no_proxy.push_str("127.0.0.1,localhost");
    let mut command=std::process::Command::new(executable);
    command.args(["--model",model]).env("ANTHROPIC_BASE_URL",origin).env("ANTHROPIC_API_KEY",token)
        .env_remove("ANTHROPIC_AUTH_TOKEN").env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC","1").env("NO_PROXY",&no_proxy).env("no_proxy",no_proxy)
        .current_dir(workspace).creation_flags(0x00000010);
    Ok(command)
}

#[tauri::command]
pub fn gateway_launch_claude(app:tauri::AppHandle,manager:tauri::State<'_,GatewayManager>,model:String,workspace:String)->Result<(),String>{
    #[cfg(windows)] {
        use tauri::Manager;
        let (router,address)={let guard=manager.0.lock().map_err(|_|"Passerelle indisponible.")?;let running=guard.as_ref().filter(|running|!running.shutdown.is_closed()).ok_or("La passerelle n’est pas active.")?;(running.router.clone(),running.status.address.clone())};
        let _provider=launch_provider(&router,&model)?;
        let folder=if workspace.trim().is_empty(){app.path().document_dir().map_err(|_|"Dossier Documents inaccessible.")?}else{std::path::PathBuf::from(workspace.trim())};
        let folder=folder.canonicalize().map_err(|_|"Dossier du projet introuvable.")?;
        if !folder.is_dir(){return Err("Choisis un dossier de projet valide.".into())}
        claude_launch_command(&claude_executable()?,&folder,&model,&address,&router.token)?.spawn().map_err(|_|"Impossible d’ouvrir la fenêtre Claude Code.".to_string())?;
        Ok(())
    }
    #[cfg(not(windows))] {let _=(app,manager,model,workspace);Err("Le lancement de Claude Code est disponible uniquement sur Windows.".into())}
}

#[tauri::command]
pub fn gateway_launch_codex(app:tauri::AppHandle,manager:tauri::State<'_,GatewayManager>,model:String,workspace:String)->Result<(),String>{
    #[cfg(windows)] {
        use tauri::Manager;
        let (router,address)={let guard=manager.0.lock().map_err(|_|"Passerelle indisponible.")?;let running=guard.as_ref().filter(|running|!running.shutdown.is_closed()).ok_or("La passerelle n’est pas active.")?;(running.router.clone(),running.status.address.clone())};
        let provider=launch_provider(&router,&model)?;
        let folder=if workspace.trim().is_empty(){app.path().document_dir().map_err(|_|"Dossier Documents inaccessible.")?}else{std::path::PathBuf::from(workspace.trim())};
        let folder=folder.canonicalize().map_err(|_|"Dossier du projet introuvable.")?;
        if !folder.is_dir(){return Err("Choisis un dossier de projet valide.".into())}
        let executable=codex_executable()?;
        codex_launch_command(&executable,&folder,&model,&address,&router.token,provider.chat_only||provider.anthropic_native)?
            .spawn().map_err(|_|"Impossible d’ouvrir la fenêtre Codex.".to_string())?;
        Ok(())
    }
    #[cfg(not(windows))] {let _=(app,manager,workspace);Err("Le lancement de Codex est disponible uniquement sur Windows.".into())}
}

#[tauri::command]
pub fn gateway_providers(manager: tauri::State<'_, GatewayManager>) -> Result<Vec<ProviderInfo>, String> {
    let router = current_router(&manager)?;
    provider_list(&router)
}

fn antigravity_upstream(provider_id:&str,credential_ref:String,account_id:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id)||account_id.is_empty()||account_id.len()>256||!account_id.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'-'|b'_')){return Err("Compte ou identifiant de routage invalide.".into())}
    let _=crate::vault::load_key(&credential_ref,"https://oauth2.googleapis.com/token",false,false)?;
    let base=upstream_url("https://daily-cloudcode-pa.googleapis.com/v1internal")?;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:true,anthropic_native:false,antigravity:Some((credential_ref,account_id)),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None}))
}

#[tauri::command]
pub fn gateway_start_antigravity(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,account_id:String,port:u16)->Result<GatewayStatus,String>{
    let upstream=antigravity_upstream(&provider_id,credential_ref,account_id)?;
    start_gateway(&manager,provider_id,upstream,port)
}

#[tauri::command]
pub fn gateway_add_antigravity(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,account_id:String)->Result<Vec<ProviderInfo>,String>{
    let upstream=antigravity_upstream(&provider_id,credential_ref,account_id)?;
    let router=current_router(&manager)?;
    {let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant de fournisseur est déjà actif.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}
    provider_list(&router)
}

fn kimi_upstream(provider_id:&str,reference:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id){return Err("Identifiant de routage invalide.".into())}
    crate::kimi_login::validate(&reference)?;
    let base=upstream_url("https://api.kimi.com/coding/v1")?;
    let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:true,anthropic_native:false,antigravity:None,kimi:Some(reference),xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None}))
}

fn nous_upstream(provider_id:&str,reference:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id){return Err("Identifiant de routage invalide.".into())}
    crate::nous_login::validate(&reference)?;
    let base=upstream_url("https://inference-api.nousresearch.com/v1")?;
    let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:Some(reference),kiro:None}))
}
#[tauri::command]pub fn gateway_start_nous(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,port:u16)->Result<GatewayStatus,String>{let upstream=nous_upstream(&provider_id,credential_ref)?;start_gateway(&manager,provider_id,upstream,port)}
#[tauri::command]pub fn gateway_add_nous(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String)->Result<Vec<ProviderInfo>,String>{let upstream=nous_upstream(&provider_id,credential_ref)?;let router=current_router(&manager)?;{let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant est déjà utilisé.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}provider_list(&router)}

fn qwen_upstream(provider_id:&str,reference:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id){return Err("Identifiant de routage invalide.".into())}
    crate::qwen_login::validate(&reference)?;
    let base=upstream_url("https://portal.qwen.ai/v1")?;
    let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:Some(reference),nous:None,kiro:None}))
}
fn kiro_upstream(provider_id:&str,reference:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id){return Err("Identifiant de fournisseur invalide.".into())}
    crate::kiro_login::validate(&reference)?;
    let base=upstream_url("https://codewhisperer.us-east-1.amazonaws.com")?;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion Kiro indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:Some(reference)}))
}
#[tauri::command]pub fn gateway_start_kiro(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,port:u16)->Result<GatewayStatus,String>{let upstream=kiro_upstream(&provider_id,credential_ref)?;start_gateway(&manager,provider_id,upstream,port)}
#[tauri::command]pub fn gateway_add_kiro(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String)->Result<Vec<ProviderInfo>,String>{let upstream=kiro_upstream(&provider_id,credential_ref)?;let router=current_router(&manager)?;{let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant est déjà utilisé.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}provider_list(&router)}
#[tauri::command]pub fn gateway_start_qwen(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,port:u16)->Result<GatewayStatus,String>{let upstream=qwen_upstream(&provider_id,credential_ref)?;start_gateway(&manager,provider_id,upstream,port)}
#[tauri::command]pub fn gateway_add_qwen(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String)->Result<Vec<ProviderInfo>,String>{let upstream=qwen_upstream(&provider_id,credential_ref)?;let router=current_router(&manager)?;{let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant est déjà utilisé.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}provider_list(&router)}

fn xai_upstream(provider_id:&str,reference:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id){return Err("Identifiant de routage invalide.".into())}
    crate::xai_login::validate(&reference)?;
    let base=upstream_url("https://cli-chat-proxy.grok.com/v1")?;
    let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:Some(reference),copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None}))
}

fn copilot_upstream(provider_id:&str,reference:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id){return Err("Identifiant de routage invalide.".into())}
    crate::copilot_login::validate(&reference)?;
    let base=upstream_url("https://api.githubcopilot.com")?;
    let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:Some(reference),chatgpt:None,qwen:None,nous:None,kiro:None}))
}
fn chatgpt_upstream(provider_id:&str,reference:String)->Result<Arc<Upstream>,String>{
    if !valid_id(provider_id){return Err("Identifiant de routage invalide.".into())}crate::openai_oauth::validate(&reference)?;
    let base=upstream_url("https://api.openai.com/v1")?;let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"Connexion réseau indisponible.")?;
    Ok(Arc::new(Upstream{base,key:String::new(),client,chat_only:false,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:Some(reference),qwen:None,nous:None,kiro:None}))
}
#[tauri::command] pub fn gateway_start_chatgpt(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,port:u16)->Result<GatewayStatus,String>{let upstream=chatgpt_upstream(&provider_id,credential_ref)?;start_gateway(&manager,provider_id,upstream,port)}
#[tauri::command] pub fn gateway_add_chatgpt(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String)->Result<Vec<ProviderInfo>,String>{let upstream=chatgpt_upstream(&provider_id,credential_ref)?;let router=current_router(&manager)?;{let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant est déjà utilisé.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}provider_list(&router)}
#[tauri::command]
pub fn gateway_start_copilot(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,port:u16)->Result<GatewayStatus,String>{let upstream=copilot_upstream(&provider_id,credential_ref)?;start_gateway(&manager,provider_id,upstream,port)}
#[tauri::command]
pub fn gateway_add_copilot(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String)->Result<Vec<ProviderInfo>,String>{
    let upstream=copilot_upstream(&provider_id,credential_ref)?;let router=current_router(&manager)?;
    {let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant est déjà utilisé.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}
    provider_list(&router)
}
#[tauri::command]
pub fn gateway_start_xai(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,port:u16)->Result<GatewayStatus,String>{let upstream=xai_upstream(&provider_id,credential_ref)?;start_gateway(&manager,provider_id,upstream,port)}
#[tauri::command]
pub fn gateway_add_xai(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String)->Result<Vec<ProviderInfo>,String>{
    let upstream=xai_upstream(&provider_id,credential_ref)?;let router=current_router(&manager)?;
    {let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant est déjà utilisé.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}
    provider_list(&router)
}
#[tauri::command]
pub fn gateway_start_kimi(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String,port:u16)->Result<GatewayStatus,String>{let upstream=kimi_upstream(&provider_id,credential_ref)?;start_gateway(&manager,provider_id,upstream,port)}
#[tauri::command]
pub fn gateway_add_kimi(manager:tauri::State<'_,GatewayManager>,provider_id:String,credential_ref:String)->Result<Vec<ProviderInfo>,String>{
    let upstream=kimi_upstream(&provider_id,credential_ref)?;let router=current_router(&manager)?;
    {let mut providers=router.providers.write().map_err(|_|"Fournisseurs indisponibles.")?;if providers.contains_key(&provider_id){return Err("Cet identifiant est déjà utilisé.".into())}if providers.len()>=32{return Err("La limite de 32 fournisseurs est atteinte.".into())}providers.insert(provider_id,upstream);}
    provider_list(&router)
}

#[tauri::command]
pub fn gateway_add_provider(manager: tauri::State<'_, GatewayManager>, provider_id: String, base_url: String, api_key: String, credential_ref: Option<String>, chat_only: bool, anthropic_native:bool) -> Result<Vec<ProviderInfo>, String> {
    if !valid_id(&provider_id) { return Err("Identifiant invalide : utilise des lettres minuscules, chiffres, tirets ou underscores.".into()); }
    if credential_ref.is_some() && !api_key.is_empty() { return Err("Choisis une clé saisie ou une clé enregistrée.".into()); }
    let key = match credential_ref { Some(reference) => crate::vault::load_key(&reference,&base_url,chat_only,anthropic_native)?, None => api_key };
    let upstream = create_upstream_with_mode(&base_url, key, chat_only,anthropic_native)?;
    let router = current_router(&manager)?;
    {
        let mut providers = router.providers.write().map_err(|_| "Fournisseurs indisponibles.")?;
        if providers.contains_key(&provider_id) { return Err("Cet identifiant est déjà utilisé.".into()); }
        if providers.len() >= 32 { return Err("La limite de 32 fournisseurs est atteinte.".into()); }
        providers.insert(provider_id, upstream);
    }
    provider_list(&router)
}

#[tauri::command]
pub fn gateway_remove_provider(manager: tauri::State<'_, GatewayManager>, provider_id: String) -> Result<Vec<ProviderInfo>, String> {
    let router = current_router(&manager)?;
    if provider_id == router.default_id { return Err("Le fournisseur par défaut ne peut pas être retiré pendant que la passerelle fonctionne.".into()); }
    {
        let mut providers = router.providers.write().map_err(|_| "Fournisseurs indisponibles.")?;
        if providers.remove(&provider_id).is_none() { return Err("Fournisseur introuvable.".into()); }
    }
    provider_list(&router)
}

#[tauri::command]
pub async fn gateway_models(manager: tauri::State<'_, GatewayManager>, provider_id: String) -> Result<Vec<String>, String> {
    let router = current_router(&manager)?;
    let upstream = router.providers.read().map_err(|_| "Fournisseurs indisponibles.")?.get(&provider_id).cloned().ok_or("Fournisseur introuvable.")?;
    let mut models: Vec<String> = fetch_catalog(upstream).await?.iter().filter_map(|entry| entry.get("id").and_then(|id| id.as_str())).map(str::to_owned).collect();
    models.sort(); models.dedup();
    Ok(models)
}

#[cfg(test)]
mod tests {
    #[test]fn cline_quota_keeps_windows_and_unknowns_separate(){let result=parse_cline_usage(&json!({"success":true,"data":{"limits":[{"type":"five_hour","percentUsed":0,"resetsAt":null},{"type":"weekly","percentUsed":72.5,"resetsAt":"2030-01-01T00:00:00Z"}]}})).unwrap();assert_eq!(result.windows.len(),2);assert_eq!(result.windows[0].percent_used,0.0);assert_eq!(result.windows[0].reset,None);assert_eq!(result.windows[1].percent_used,72.5);assert!(parse_cline_usage(&json!({"success":false})).is_err());assert!(parse_cline_usage(&json!({"success":true,"data":{"limits":[{"type":"weekly","percentUsed":10},{"type":"weekly","percentUsed":20}]}})).is_err());}
    #[tokio::test]async fn cline_quota_uses_key_for_the_selected_account(){let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=Mutex::new(Some(tx));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.lock().unwrap().take();async move{tx.unwrap().send(request.headers()["authorization"].to_str().unwrap().to_owned()).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"success":true,"data":{"limits":[{"type":"weekly","percentUsed":30}]}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});let client=reqwest::Client::builder().no_proxy().build().unwrap();let usage=fetch_cline_usage(&client,format!("http://{addr}/limits").parse().unwrap(),"synthetic-key").await.unwrap();assert_eq!(usage.windows[0].percent_used,30.0);assert_eq!(rx.await.unwrap(),"Bearer synthetic-key");}
    #[tokio::test]async fn cline_responses_use_openai_header_and_preserve_model_slug(){
        let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let tx=Mutex::new(Some(tx));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.lock().unwrap().take();async move{let path=request.uri().path().to_owned();let format=request.headers().get("x-response-format").and_then(|v|v.to_str().ok()).unwrap_or("").to_owned();let auth=request.headers().get("authorization").and_then(|v|v.to_str().ok()).unwrap_or("").to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();tx.unwrap().send((path,format,auth,body)).unwrap();Ok::<_,Infallible>(Response::builder().header("content-type","application/json").body(Full::new(Bytes::from_static(br#"{"choices":[{"index":0,"message":{"role":"assistant","content":"Bonjour"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=create_upstream(&format!("http://{upstream_addr}/__nivra_test_cline/v1"),"synthetic-cline-key".into(),true).unwrap();let router=Arc::new(Router{token:"local-key".into(),default_id:"cline".into(),providers:RwLock::new(BTreeMap::from([("cline".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/responses")).bearer_auth("local-key").body(json!({"model":"cline/cline-pass/example","input":"Salut"}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let output:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();assert_eq!(output["output"][0]["content"][0]["text"],"Bonjour");let(path,format,auth,body)=rx.await.unwrap();assert_eq!(path,"/__nivra_test_cline/v1/chat/completions");assert_eq!(format,"openai");assert_eq!(auth,"Bearer synthetic-cline-key");let body:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["model"],"cline-pass/example");assert_eq!(body["stream"],false);
    }
    #[tokio::test]async fn cline_catalog_reads_live_recommendations_without_sending_key(){let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=Mutex::new(Some(tx));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.lock().unwrap().take();async move{tx.unwrap().send(request.headers().contains_key("authorization")).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"recommended":[{"id":"anthropic/example"}],"free":[{"id":"anthropic/example"}],"clinePass":[{"id":"cline-pass/example"}]}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});let client=reqwest::Client::builder().no_proxy().build().unwrap();let models=fetch_cline_catalog(&client,format!("http://{addr}/catalog").parse().unwrap()).await.unwrap();assert_eq!(models.len(),2);assert_eq!(models[0]["id"],"anthropic/example");assert_eq!(models[1]["id"],"cline-pass/example");assert!(!rx.await.unwrap());}
    #[test]fn cline_defaults_to_non_streaming_and_restricts_host(){let body=cline_chat_body(&Bytes::from_static(br#"{"model":"m","messages":[]}"#)).unwrap();assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["stream"],false);let body=cline_chat_body(&Bytes::from_static(br#"{"stream":true}"#)).unwrap();assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["stream"],true);assert!(cline_chat_body(&Bytes::from_static(br#"{"stream":"true"}"#)).is_err());assert!(is_cline(&create_upstream("https://api.cline.bot/api/v1","synthetic".into(),true).unwrap()));assert!(!is_cline(&create_upstream("https://api.cline.bot.evil.test/api/v1","synthetic".into(),true).unwrap()));}
    #[tokio::test]async fn kiro_gateway_replays_signed_tool_reasoning(){
        let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,mut rx)=tokio::sync::mpsc::channel(2);tauri::async_runtime::spawn(async move{for index in 0..2{let(socket,_)=upstream_listener.accept().await.unwrap();let tx=tx.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{tx.send(request.into_body().collect().await.unwrap().to_bytes()).await.unwrap();let mut wire=Vec::new();if index==0{wire.extend(crate::kiro_eventstream::tests::frame("reasoningContentEvent",br#"{"text":"secret","signature":"sig"}"#));wire.extend(crate::kiro_eventstream::tests::frame("toolUseEvent",br#"{"toolUseId":"call","name":"read","input":"{}","stop":true}"#));wire.extend(crate::kiro_eventstream::tests::frame("metadataEvent",br#"{"stopReason":"TOOL_USE"}"#));}else{wire.extend(crate::kiro_eventstream::tests::frame("assistantResponseEvent",br#"{"content":"Termine"}"#));wire.extend(crate::kiro_eventstream::tests::frame("metadataEvent",br#"{"stopReason":"END_TURN"}"#));}Ok::<_,Infallible>(Response::builder().header("content-type","application/vnd.amazon.eventstream").body(Full::new(Bytes::from(wire))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:Some("synthetic-kiro-test".into())});let router=Arc::new(Router{token:"tool-test-key".into(),default_id:"kiro".into(),providers:RwLock::new(BTreeMap::from([("kiro".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{for _ in 0..2{let(socket,_)=listener.accept().await.unwrap();let router=router.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let url=format!("http://{address}/v1/chat/completions");let tools=json!([{"type":"function","function":{"name":"read","parameters":{"type":"object"}}}]);let mut input=json!({"model":"kiro/model-a","messages":[{"role":"user","content":"Lis"}],"tools":tools});let first=client.post(&url).bearer_auth("tool-test-key").body(input.to_string()).send().await.unwrap();assert_eq!(first.status(),StatusCode::OK);let first:serde_json::Value=serde_json::from_slice(&first.bytes().await.unwrap()).unwrap();assert!(first.get("_nivra_kiro_reasoning").is_none());input["messages"].as_array_mut().unwrap().push(first["choices"][0]["message"].clone());input["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":"call","content":"ok"}));let next=client.post(&url).bearer_auth("tool-test-key").body(input.to_string()).send().await.unwrap();assert_eq!(next.status(),StatusCode::OK);let _=rx.recv().await.unwrap();let body:serde_json::Value=serde_json::from_slice(&rx.recv().await.unwrap()).unwrap();assert_eq!(body["conversationState"]["history"][1]["assistantResponseMessage"]["reasoningContent"]["reasoningText"]["signature"],"sig");
    }
    #[tokio::test]async fn messages_errors_distinguish_rate_limits(){let response=messages_error(StatusCode::TOO_MANY_REQUESTS,"limite");let bytes=response.into_body().collect().await.unwrap().to_bytes();let body:serde_json::Value=serde_json::from_slice(&bytes).unwrap();assert_eq!(body["error"]["type"],"rate_limit_error");}
    #[tokio::test]
    async fn kiro_gateway_preserves_rate_limit_and_retry_after(){
        let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let service=service_fn(|_request:Request<Incoming>|async move{Ok::<_,Infallible>(Response::builder().status(429).header("retry-after","35").body(Full::new(Bytes::from_static(b"redacted"))).unwrap())});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:Some("synthetic-kiro-test".into())});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"kiro".into(),providers:RwLock::new(BTreeMap::from([("kiro".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/chat/completions")).bearer_auth("local-gateway-key").body(json!({"model":"kiro/model-a","messages":[{"role":"user","content":"salut"}]}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::TOO_MANY_REQUESTS);assert_eq!(response.headers()["retry-after"],"35");assert!(!response.text().await.unwrap().contains("redacted"));
    }
    #[tokio::test]
    async fn kiro_gateway_translates_messages_with_model_limit(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=listener.local_addr().unwrap();let(tx,mut rx)=tokio::sync::mpsc::channel(2);
        tauri::async_runtime::spawn(async move{for _ in 0..2{let(socket,_)=listener.accept().await.unwrap();let tx=tx.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();tx.send((path.clone(),body)).await.unwrap();let (kind,body)=if path=="/ListAvailableModels"{("application/json",Bytes::from_static(br#"{"models":[{"modelId":"model-a","additionalModelRequestFieldsSchema":{"properties":{"max_tokens":{"type":"integer","minimum":1024,"maximum":128000}}}}]}"#))}else{let mut wire=crate::kiro_eventstream::tests::frame("assistantResponseEvent",br#"{"content":"Bonjour"}"#);wire.extend(crate::kiro_eventstream::tests::frame("meteringEvent",br#"{"unit":"credit","usage":0.01}"#));("application/vnd.amazon.eventstream",Bytes::from(wire))};Ok::<_,Infallible>(Response::builder().header("content-type",kind).body(Full::new(body)).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:Some("synthetic-kiro-test".into())});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"kiro".into(),providers:RwLock::new(BTreeMap::from([("kiro".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/messages")).header("x-api-key","local-gateway-key").body(json!({"model":"kiro/model-a","max_tokens":2048,"messages":[{"role":"user","content":"salut"}]}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()["x-nivra-usage-source"],"estimated");assert_eq!(response.headers()["x-nivra-kiro-finish"],"inferred-from-metering");let output:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();assert_eq!(output["content"][0]["text"],"Bonjour");assert!(output["usage"]["input_tokens"].as_u64().unwrap()>0);let(first,_)=rx.recv().await.unwrap();let(second,body)=rx.recv().await.unwrap();assert_eq!(first,"/ListAvailableModels");assert_eq!(second,"/generateAssistantResponse");let body:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["additionalModelRequestFields"]["max_tokens"],2048);
    }
    #[tokio::test]
    async fn kiro_gateway_routes_streaming_responses_through_real_translation(){
        let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,path,body)).unwrap();let mut wire=crate::kiro_eventstream::tests::frame("assistantResponseEvent",br#"{"content":"Bonjour"}"#);wire.extend(crate::kiro_eventstream::tests::frame("metadataEvent",br#"{"stopReason":"END_TURN"}"#));Ok::<_,Infallible>(Response::builder().header("content-type","application/vnd.amazon.eventstream").body(Full::new(Bytes::from(wire))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:Some("synthetic-kiro-test".into())});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"kiro".into(),providers:RwLock::new(BTreeMap::from([("kiro".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/responses")).bearer_auth("local-gateway-key").body(json!({"model":"kiro/model-a","instructions":"Aide","input":[{"role":"user","content":[{"type":"input_text","text":"salut"},{"type":"input_image","image_url":"data:image/png;base64,AA=="}]}],"stream":true}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let output=response.text().await.unwrap();assert!(output.contains("response.completed"));assert!(output.contains("Bonjour"));let(auth,path,body)=rx.await.unwrap();assert_eq!(auth,"Bearer synthetic-kiro-access");assert_eq!(path,"/generateAssistantResponse");let sent:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(sent.pointer("/conversationState/currentMessage/userInputMessage/modelId").unwrap(),"model-a");assert!(sent.pointer("/conversationState/currentMessage/userInputMessage/content").unwrap().as_str().unwrap().contains("Aide"));assert_eq!(sent.pointer("/conversationState/currentMessage/userInputMessage/images/0/source/bytes").unwrap(),"AA==");
    }
    #[cfg(windows)]#[tokio::test]#[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
    async fn nous_gateway_uses_account_inference_endpoint(){
        use serde_json::Value;
        struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
        let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        let reference=crate::nous_login::synthetic_cached_session(upstream_url(&format!("http://{upstream_addr}/v1")).unwrap()).await;let _cleanup=Cleanup(reference.clone());
        tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,path,body)).unwrap();let answer=json!({"choices":[{"index":0,"message":{"role":"assistant","content":"Bonjour"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}});Ok::<_,Infallible>(Response::builder().header("content-type","application/json").body(Full::new(Bytes::from(answer.to_string()))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url("https://inference-api.nousresearch.com/v1").unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:Some(reference),kiro:None});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"nous".into(),providers:RwLock::new(BTreeMap::from([("nous".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/responses")).bearer_auth("local-gateway-key").body(json!({"model":"nous/org/model","input":"salut","stream":false}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let result:Value=serde_json::from_str(&response.text().await.unwrap()).unwrap();assert_eq!(result["output"][0]["content"][0]["text"],"Bonjour");let(auth,path,body)=rx.await.unwrap();assert_eq!(auth,"Bearer synthetic-nous-access");assert_eq!(path,"/v1/chat/completions");let sent:Value=serde_json::from_slice(&body).unwrap();assert_eq!(sent["model"],"org/model");
    }
    #[test]
    fn antigravity_chat_accepts_inline_images_and_rejects_external_or_invalid_images(){
        use serde_json::json;
        let input=json!({"model":"gemini","messages":[{"role":"user","content":[{"type":"text","text":"Décris"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}},{"type":"text","text":"merci"}]}]});
        let request=super::simple_antigravity_chat(&input).unwrap().1;assert_eq!(request["contents"][0]["parts"][0]["text"],"Décris");assert_eq!(request["contents"][0]["parts"][1]["inlineData"]["mimeType"],"image/png");assert_eq!(request["contents"][0]["parts"][1]["inlineData"]["data"],"AA==");assert_eq!(request["contents"][0]["parts"][2]["text"],"merci");
        for url in ["https://example.com/a.png","data:image/png;base64,***","data:text/html;base64,AA=="]{let mut bad=input.clone();bad["messages"][0]["content"][1]["image_url"]["url"]=json!(url);assert!(super::simple_antigravity_chat(&bad).is_err());}
        let mut detail=input.clone();detail["messages"][0]["content"][1]["image_url"]["detail"]=json!("high");assert!(super::simple_antigravity_chat(&detail).is_err());
    }
    #[test]
    fn antigravity_accepts_inline_images_from_responses_and_messages(){
        use serde_json::json;
        let response=json!({"model":"gemini","input":[{"role":"user","content":[{"type":"input_text","text":"Décris"},{"type":"input_image","image_url":"data:image/jpeg;base64,AA=="}]}]});
        let request=super::simple_antigravity_responses(&response).unwrap().1;assert_eq!(request["contents"][0]["parts"][1]["inlineData"]["mimeType"],"image/jpeg");
        let messages=json!({"model":"gemini","max_tokens":100,"messages":[{"role":"user","content":[{"type":"text","text":"Décris"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}]});
        let request=super::simple_antigravity_messages(&messages).unwrap().1;assert_eq!(request["contents"][0]["parts"][1]["inlineData"]["data"],"AA==");
        let mut bad=messages;bad["messages"][0]["content"][1]["source"]["type"]=json!("url");assert!(super::simple_antigravity_messages(&bad).is_err());
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
    async fn qwen_gateway_routes_account_responses_to_chat(){
        struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
        let reference=crate::qwen_login::synthetic_cached_session().await;let _cleanup=Cleanup(reference.clone());let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let kind=request.headers()["x-dashscope-authtype"].to_str().unwrap().to_owned();let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,kind,path,body)).unwrap();let answer=serde_json::json!({"choices":[{"index":0,"message":{"role":"assistant","content":"Bonjour"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}});Ok::<_,Infallible>(Response::builder().header("content-type","application/json").body(Full::new(Bytes::from(answer.to_string()))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/v1")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:Some(reference),nous:None,kiro:None});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"qwen".into(),providers:RwLock::new(BTreeMap::from([("qwen".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/responses")).bearer_auth("local-gateway-key").body(serde_json::json!({"model":"qwen/coder-model","input":"salut","stream":false}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let result:serde_json::Value=serde_json::from_str(&response.text().await.unwrap()).unwrap();assert_eq!(result["output"][0]["content"][0]["text"],"Bonjour");let(auth,kind,path,body)=rx.await.unwrap();assert_eq!(auth,"Bearer synthetic-qwen-access");assert_eq!(kind,"qwen-oauth");assert_eq!(path,"/v1/chat/completions");let sent:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(sent["model"],"coder-model");
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
    async fn grok_gateway_buffers_non_stream_requests_and_preserves_model_headers(){
        struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
        let reference=crate::xai_login::synthetic_cached_session().await;let _cleanup=Cleanup(reference.clone());
        let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,mut rx)=tokio::sync::mpsc::channel(2);
        tauri::async_runtime::spawn(async move{for _ in 0..2{let(socket,_)=upstream_listener.accept().await.unwrap();let tx=tx.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let mode=request.headers()["x-xai-token-auth"].to_str().unwrap().to_owned();let model=request.headers()["x-grok-model-override"].to_str().unwrap().to_owned();let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();tx.send((auth,mode,model,path,body)).await.unwrap();let answer="data: {\"id\":\"grok-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Bon\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"jour\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\ndata: [DONE]\n\n";Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from(answer))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/v1")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:Some(reference),copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"grok".into(),providers:RwLock::new(BTreeMap::from([("grok".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{for _ in 0..2{let(socket,_)=listener.accept().await.unwrap();let router=router.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();
        for (route,input) in [("chat/completions",serde_json::json!({"model":"grok/grok-4.6","messages":[{"role":"user","content":"salut"}]})),("responses",serde_json::json!({"model":"grok/grok-4.6","input":"salut","stream":false}))]{
            let response=client.post(format!("http://{address}/v1/{route}")).bearer_auth("local-gateway-key").body(input.to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let result:serde_json::Value=serde_json::from_str(&response.text().await.unwrap()).unwrap();
            if route=="responses"{assert_eq!(result["output"][0]["content"][0]["text"],"Bonjour")}else{assert_eq!(result["choices"][0]["message"]["content"],"Bonjour")}
            let(auth,mode,model,path,body)=rx.recv().await.unwrap();assert_eq!(auth,"Bearer synthetic-grok-access");assert_eq!(mode,"xai-grok-cli");assert_eq!(model,"grok-4.6");assert_eq!(path,"/v1/chat/completions");let sent:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(sent["model"],"grok-4.6");assert_eq!(sent["stream"],true);
        }
    }
    #[test]
    fn grok_auth_and_model_headers_follow_selected_model() {
        let client=reqwest::Client::new();
        let request=super::grok_headers(client.post("https://example.test/v1/chat/completions").bearer_auth("synthetic"),Some("grok-4.6")).unwrap().build().unwrap();
        assert_eq!(request.headers()["x-xai-token-auth"],"xai-grok-cli");
        assert_eq!(request.headers()["x-grok-model-override"],"grok-4.6");
        assert_eq!(request.headers()["authorization"],"Bearer synthetic");
        let catalog=super::grok_headers(client.get("https://example.test/v1/models"),None).unwrap().build().unwrap();
        assert_eq!(catalog.headers()["x-xai-token-auth"],"xai-grok-cli");
        assert!(!catalog.headers().contains_key("x-grok-model-override"));
        assert!(super::grok_headers(client.post("https://example.test"),Some("bad\r\nheader")).is_err());
    }
    #[test]
    fn chatgpt_plan_requires_stream_without_storage_and_filters_catalog(){
        use serde_json::json;use bytes::Bytes;
        assert!(super::chatgpt_request(&Bytes::from_static(br#"{"model":"x","stream":true,"store":false}"#)).is_ok());
        for data in [br#"{"stream":false,"store":false}"#.as_slice(),br#"{"stream":true}"#.as_slice(),br#"{"stream":true,"store":true}"#.as_slice()]{assert!(super::chatgpt_request(&Bytes::copy_from_slice(data)).is_err())}
        let result=super::parse_chatgpt_catalog(&json!({"models":[{"slug":"visible","visibility":"list"},{"slug":"hidden","visibility":"hide"},{"slug":"bad\nname","visibility":"list"}]})).unwrap();assert_eq!(result.len(),1);assert_eq!(result[0]["id"],"visible");assert!(super::parse_chatgpt_catalog(&json!({"data":[]})).is_err());
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
    async fn chatgpt_gateway_forwards_stream_with_account_credential(){
        struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
        let reference=crate::openai_oauth::synthetic_cached_session().await;let _cleanup=Cleanup(reference.clone());
        let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((auth,path,body)).unwrap();Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from_static(b"data: {\"type\":\"response.completed\"}\n\n"))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/v1")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:false,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:Some(reference),qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"chatgpt".into(),providers:RwLock::new(BTreeMap::from([("chatgpt".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{for _ in 0..3{let(socket,_)=listener.accept().await.unwrap();let router=router.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let client=reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();let base=format!("http://{address}/v1");
        assert_eq!(client.post(format!("{base}/responses")).body(serde_json::json!({"model":"chatgpt/example","stream":true,"store":false}).to_string()).send().await.unwrap().status(),StatusCode::UNAUTHORIZED);
        assert_eq!(client.post(format!("{base}/chat/completions")).bearer_auth("local-gateway-key").body(serde_json::json!({"model":"chatgpt/example","stream":true,"messages":[],"temperature":0.5}).to_string()).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
        let response=client.post(format!("{base}/responses")).bearer_auth("local-gateway-key").body(serde_json::json!({"model":"chatgpt/example","stream":true,"store":false,"input":"hello"}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);assert!(response.text().await.unwrap().contains("response.completed"));let(auth,path,body)=rx.await.unwrap();assert_eq!(auth,"Bearer synthetic-openai-access");assert_eq!(path,"/v1/responses");let sent:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(sent["model"],"example");assert_eq!(sent["store"],false);
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
    async fn chatgpt_gateway_translates_chat_to_responses(){
        struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
        let reference=crate::openai_oauth::synthetic_cached_session().await;let _cleanup=Cleanup(reference.clone());let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let path=request.uri().path().to_owned();let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send((path,body)).unwrap();let answer="data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Bonjour\"}]}]}}\n\n";Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from(answer))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/v1")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:false,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:Some(reference),qwen:None,nous:None,kiro:None});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"chatgpt".into(),providers:RwLock::new(BTreeMap::from([("chatgpt".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/chat/completions")).bearer_auth("local-gateway-key").body(serde_json::json!({"model":"chatgpt/example","messages":[{"role":"user","content":"bonjour"}]}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let result:serde_json::Value=serde_json::from_str(&response.text().await.unwrap()).unwrap();assert_eq!(result["choices"][0]["message"]["content"],"Bonjour");let(path,body)=rx.await.unwrap();assert_eq!(path,"/v1/responses");let sent:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(sent["model"],"example");assert_eq!(sent["stream"],true);assert_eq!(sent["store"],false);assert_eq!(sent["input"][0]["content"],"bonjour");
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
    async fn chatgpt_gateway_translates_messages_with_tool(){
        struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
        let reference=crate::openai_oauth::synthetic_cached_session().await;let _cleanup=Cleanup(reference.clone());let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let sender=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let sender=sender.clone();async move{let body=request.into_body().collect().await.unwrap().to_bytes();sender.lock().unwrap().take().unwrap().send(body).unwrap();let answer="data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"call1\",\"name\":\"read\",\"arguments\":\"{}\"}],\"usage\":{\"input_tokens\":5,\"output_tokens\":3}}}\n\n";Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from(answer))).unwrap())}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/v1")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:false,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:Some(reference),qwen:None,nous:None,kiro:None});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"chatgpt".into(),providers:RwLock::new(BTreeMap::from([("chatgpt".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let response=client.post(format!("http://{address}/v1/messages")).header("x-api-key","local-gateway-key").body(serde_json::json!({"model":"chatgpt/example","max_tokens":100,"stream":true,"messages":[{"role":"user","content":[{"type":"text","text":"lis"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}],"tools":[{"name":"read","input_schema":{"type":"object"}}]}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let stream=response.text().await.unwrap();assert!(stream.contains("event: message_start"));assert!(stream.contains("\"name\":\"read\""));assert!(stream.contains("event: message_stop"));let body=rx.await.unwrap();let sent:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(sent["model"],"example");assert_eq!(sent["tools"][0]["name"],"read");assert_eq!(sent["input"][0]["content"][1]["image_url"],"data:image/png;base64,AA==");assert_eq!(sent["stream"],true);assert_eq!(sent["store"],false);
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Creates and removes a synthetic Windows Credential Manager entry"]
    async fn chatgpt_gateway_streams_chat_text_before_completion(){
        struct Cleanup(String);impl Drop for Cleanup{fn drop(&mut self){let _=crate::vault::gateway_forget_key(self.0.clone());}}
        let reference=crate::openai_oauth::synthetic_cached_session().await;let _cleanup=Cleanup(reference.clone());let upstream_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_addr=upstream_listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{let(socket,_)=upstream_listener.accept().await.unwrap();let service=service_fn(|_:Request<Incoming>|async{let body=StreamBody::new(stream::unfold(0,|step|async move{match step{0=>Some((Ok::<_,Infallible>(Frame::data(Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"Bon\"}\n\n"))),1)),1=>{tokio::time::sleep(Duration::from_millis(800)).await;Some((Ok(Frame::data(Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Bonjour\"}]}]}}\n\n"))),2))},_=>None}}));Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(body).unwrap())});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=Arc::new(Upstream{base:upstream_url(&format!("http://{upstream_addr}/v1")).unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:false,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:Some(reference),qwen:None,nous:None,kiro:None});let router=Arc::new(Router{token:"local-gateway-key".into(),default_id:"chatgpt".into(),providers:RwLock::new(BTreeMap::from([("chatgpt".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let mut response=client.post(format!("http://{address}/v1/chat/completions")).bearer_auth("local-gateway-key").body(serde_json::json!({"model":"chatgpt/example","stream":true,"messages":[{"role":"user","content":"bonjour"}]}).to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let first=tokio::time::timeout(Duration::from_millis(500),async{let mut output=String::new();while !output.contains("Bon"){let chunk=response.chunk().await.unwrap().unwrap();output.push_str(std::str::from_utf8(&chunk).unwrap());}output}).await.expect("text should arrive before final response");assert!(first.contains("Bon"));let rest=response.text().await.unwrap();assert!(rest.contains("jour"));assert!(rest.contains("[DONE]"));
    }
    #[test]
    fn opencode_go_usage_accepts_real_windows_without_inventing_them(){
        use serde_json::json;
        let result=super::parse_opencode_go_usage(&json!({"usage":{"rolling":{"status":"ok","percent":1,"resetsAt":"2026-12-01T00:00:00Z"},"weekly":{"status":"rate-limited","percent":100,"resetsAt":"2026-12-07T00:00:00Z"}}})).unwrap();assert_eq!(result.windows.len(),2);assert_eq!(result.windows[0].percent_used,1.0);assert_eq!(result.windows[1].status,"rate-limited");
        assert!(super::parse_opencode_go_usage(&json!({"usage":{"rolling":{"status":"ok","percent":-1,"resetsAt":"2026-12-01T00:00:00Z"}}})).is_err());assert!(super::parse_opencode_go_usage(&json!({"usage":{"rolling":{"status":"ok","percent":1,"resetsAt":"bad"}}})).is_err());
        let good=super::create_upstream("https://opencode.ai/zen/go/v1","synthetic".into(),true).unwrap();assert!(super::is_opencode_go(&good));let bad=super::create_upstream("https://opencode.ai.evil.example/zen/go/v1","synthetic".into(),true).unwrap();assert!(!super::is_opencode_go(&bad));assert!(super::opencode_go_responses_model("grok-4.6"));assert!(!super::opencode_go_responses_model("kimi-k3"));assert!(super::anthropic_wire_for(&good,Some("minimax-m3")));assert!(super::anthropic_wire_for(&good,Some("qwen3.8-max")));assert!(!super::anthropic_wire_for(&good,Some("kimi-k3")));assert!(!super::anthropic_wire_for(&bad,Some("minimax-m3")));
    }
    #[tokio::test]
    async fn opencode_go_usage_reads_simulated_authenticated_response(){
        use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();tx.lock().unwrap().take().unwrap().send(auth).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"usage":{"rolling":{"status":"ok","percent":1,"resetsAt":"2026-12-01T00:00:00Z"}}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=std::sync::Arc::new(super::Upstream{base:super::upstream_url("https://opencode.ai/zen/go/v1").unwrap(),key:"synthetic".into(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let result=super::fetch_opencode_go_usage(upstream,format!("http://{addr}/usage").parse().unwrap()).await.unwrap();assert_eq!(result.windows[0].percent_used,1.0);assert_eq!(rx.await.unwrap(),"Bearer synthetic");
    }
    #[test]
    fn glm_quota_respects_exact_hosts_and_preserves_distinct_windows(){
        use serde_json::json;
        let zai=super::create_upstream("https://api.z.ai/api/coding/paas/v4","x".into(),true).unwrap();assert_eq!(super::glm_region(&zai),Some("zai"));
        let cn=super::create_upstream("https://open.bigmodel.cn/api/coding/paas/v4","x".into(),true).unwrap();assert_eq!(super::glm_region(&cn),Some("bigmodel"));
        for url in ["https://api.z.ai.evil.example/api/coding/paas/v4","https://api.z.ai/api/paas/v4","https://open.bigmodel.cn/other"]{let upstream=super::create_upstream(url,"x".into(),true).unwrap();assert_eq!(super::glm_region(&upstream),None);}
        let result=super::parse_glm_quota(&json!({"data":{"limits":[{"type":"TIME_LIMIT","unit":5,"usage":100,"currentValue":0,"percentage":0},{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":1,"nextResetTime":1786626122911u64},{"type":"UNKNOWN","percentage":50}]}})).unwrap();assert_eq!(result.windows.len(),2);assert_eq!(result.windows[0].kind,"TIME_LIMIT");assert_eq!(result.windows[0].used,Some(0.0));assert_eq!(result.windows[1].unit,Some(3));assert_eq!(result.windows[1].reset_ms,Some(1786626122911));
        assert!(super::parse_glm_quota(&json!({"success":false})).is_err());assert!(super::parse_glm_quota(&json!({"data":{"limits":[]}})).unwrap().windows.is_empty());
    }
    #[tokio::test]
    async fn glm_quota_sends_region_specific_authorization(){
        use hyper::{body::Incoming,service::service_fn,Request,Response};use hyper_util::rt::TokioIo;use http_body_util::Full;use bytes::Bytes;
        for(region,expected) in [("zai","Bearer synthetic"),("bigmodel","synthetic")]{
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
            tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let auth=request.headers()["authorization"].to_str().unwrap().to_owned();tx.lock().unwrap().take().unwrap().send(auth).unwrap();Ok::<_,std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"percentage":25}]}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
            let upstream=std::sync::Arc::new(super::Upstream{base:super::upstream_url("https://example.com/v1").unwrap(),key:"synthetic".into(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
            let quota=super::fetch_glm_quota(upstream,format!("http://{addr}/quota").parse().unwrap(),region).await.unwrap();assert_eq!(quota.windows[0].percent_used,Some(25.0));assert_eq!(rx.await.unwrap(),expected);
        }
    }
    #[test]
    fn copilot_known_responses_models_use_native_protocol(){
        assert!(super::copilot_responses_model("gpt-5.4"));
        assert!(super::copilot_responses_model("gpt-6.1-terra"));
        assert!(super::copilot_responses_model("grok-4.5"));
        assert!(!super::copilot_responses_model("claude-sonnet-4"));
        assert!(!super::copilot_responses_model("gpt-5.4-unrecognized"));
    }
    use super::*;
    use serde_json::json;
    #[tokio::test] async fn antigravity_gateway_can_start_as_default_without_an_api_provider(){
        let temporary=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let port=temporary.local_addr().unwrap().port();drop(temporary);
        let manager=GatewayManager::default();let client=reqwest::Client::builder().no_proxy().build().unwrap();
        let upstream=Arc::new(Upstream{base:upstream_url("https://daily-cloudcode-pa.googleapis.com/v1internal").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-a".into(),"account-a".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let status=start_gateway(&manager,"antigravity".into(),upstream.clone(),port).unwrap();
        assert!(start_gateway(&manager,"antigravity".into(),upstream,port).is_err());
        let router=manager.0.lock().unwrap().as_ref().unwrap().router.clone();let list=provider_list(&router).unwrap();assert_eq!(list.len(),1);assert!(list[0].is_default&&list[0].antigravity);
        let input=json!({"model":"model-a","messages":[{"role":"user","content":"Salut"}]});let (selected,body)=select_provider(&router,&Bytes::from(input.to_string())).unwrap();assert_eq!(selected.antigravity.as_ref().unwrap().1,"account-a");assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["model"],"model-a");
        let denied=client.post(format!("{}/chat/completions",status.address)).bearer_auth("wrong").body(input.to_string()).send().await.unwrap();assert_eq!(denied.status(),StatusCode::UNAUTHORIZED);
        let running=manager.0.lock().unwrap().take().unwrap();running.shutdown.send(true).unwrap();
    }
    #[test] fn antigravity_stream_parses_split_utf8_and_detects_errors(){
        let mut stream=GoogleChatStream::new("agy/m".into());
        let data=format!("data: {}\n\ndata: {}\n\n",json!({"response":{"candidates":[{"content":{"parts":[{"text":"Été"}]}}]}}),json!({"response":{"candidates":[{"finishReason":"MAX_TOKENS"}]}}));
        let bytes=data.as_bytes();let split=bytes.iter().position(|b|*b==0xc3).unwrap()+1;assert!(stream.ingest(&bytes[..split]).unwrap().is_empty());let out=stream.ingest(&bytes[split..]).unwrap();let result=out.iter().map(|b|std::str::from_utf8(b).unwrap()).collect::<String>();assert!(result.contains("Été"));assert!(result.contains("\"finish_reason\":\"length\""));assert!(stream.done);
        let mut stream=GoogleChatStream::new("m".into());assert!(stream.ingest(format!("data: {}\n\n",json!({"response":{"candidates":[{"finishReason":"SAFETY"}]}})).as_bytes()).is_err());
        let mut stream=GoogleChatStream::new("m".into());assert!(stream.ingest(format!("data: {}\n\n",json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"name":"x"}}]}}]}})).as_bytes()).is_err());
    }
    #[tokio::test] async fn antigravity_stream_delivers_text_before_upstream_finishes(){
        for responses in [false,true]{
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let (release_tx,release_rx)=tokio::sync::oneshot::channel::<()>();let release=Arc::new(Mutex::new(Some(release_rx)));
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|_request:Request<Incoming>|{let release=release.lock().unwrap().take().unwrap();async move{
            let first=Bytes::from(format!("data: {}\n\n",json!({"response":{"candidates":[{"content":{"parts":[{"text":"premier"}]}}]}})));
            let last=Bytes::from(format!("data: {}\n\n",json!({"response":{"candidates":[{"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":8,"toolUsePromptTokenCount":2,"candidatesTokenCount":3,"thoughtsTokenCount":4}}})));
            let chunks=stream::unfold((0,Some(release),first,last),|(step,mut release,first,last)|async move{match step{0=>Some((Ok::<_,std::io::Error>(Frame::data(first.clone())),(1,release,first,last))),1=>{let _=release.take().unwrap().await;Some((Ok(Frame::data(last.clone())),(2,release,first,last)))},_=>None}});
            Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(StreamBody::new(chunks)).unwrap())
        }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let received=client.get(format!("http://{address}/")).send().await.unwrap();let mut body=antigravity_stream_response(received,"agy/m".into(),responses).into_body();
        let mut collected=String::new();while !collected.contains("premier"){let frame=tokio::time::timeout(Duration::from_secs(2),body.frame()).await.unwrap().unwrap().unwrap();if let Ok(bytes)=frame.into_data(){collected.push_str(std::str::from_utf8(&bytes).unwrap());}}
        release_tx.send(()).unwrap();while let Some(frame)=body.frame().await{let frame=frame.unwrap();if let Ok(bytes)=frame.into_data(){collected.push_str(std::str::from_utf8(&bytes).unwrap());}}
        if responses {assert!(collected.contains("response.incomplete"));assert!(collected.contains("\"reason\":\"max_output_tokens\""));assert!(collected.contains("\"input_tokens\":10"));assert!(collected.contains("\"output_tokens\":7"));}else{assert!(collected.contains("[DONE]"));assert!(collected.contains("\"finish_reason\":\"length\""));assert!(collected.contains("\"prompt_tokens\":10"));assert!(collected.contains("\"completion_tokens\":7"));}
        }
    }
    #[tokio::test] async fn antigravity_responses_stream_reports_interrupted_upstream(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(|_request:Request<Incoming>|async move{let data=format!("data: {}\n\n",json!({"response":{"candidates":[{"content":{"parts":[{"text":"partiel"}]}}]}}));Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from(data))).unwrap())});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let received=client.get(format!("http://{address}/")).send().await.unwrap();let result=antigravity_stream_response(received,"agy/m".into(),true).into_body().collect().await.unwrap().to_bytes();let result=std::str::from_utf8(&result).unwrap();assert!(result.contains("partiel"));assert!(result.contains("response.failed"));assert!(!result.contains("response.completed"));
    }
    #[test] fn antigravity_gateway_refuses_unhandled_chat_features(){
        let basic=json!({"model":"model-a","messages":[{"role":"user","content":"Bonjour"}],"stream":false});assert_eq!(simple_antigravity_chat(&basic).unwrap().1["contents"][0]["parts"][0]["text"],"Bonjour");
        let streaming=json!({"model":"model-a","messages":[{"role":"user","content":"Bonjour"}],"stream":true});assert!(simple_antigravity_chat(&streaming).unwrap().2);
        let tools=json!({"model":"model-a","messages":[{"role":"user","content":"Bonjour"}],"tools":[]});assert!(simple_antigravity_chat(&tools).is_ok());assert!(simple_antigravity_chat(&json!({"model":"model-a","messages":[{"role":"user","content":"Bonjour"}],"tools":[{"type":"function"}]})).is_err());
        let history=json!({"model":"model-a","messages":[{"role":"system","content":"Guide"},{"role":"user","content":"a"},{"role":"assistant","content":"b"},{"role":"user","content":"c"}]});let converted=simple_antigravity_chat(&history).unwrap().1;assert_eq!(converted["systemInstruction"]["parts"][0]["text"],"Guide");assert_eq!(converted["contents"][1]["role"],"model");assert_eq!(converted["contents"][2]["parts"][0]["text"],"c");assert!(simple_antigravity_chat(&json!({"model":"m","messages":[{"role":"assistant","content":"a"}]})).is_err());
    }
    #[test] fn antigravity_sampling_controls_are_preserved_and_validated(){
        let chat=json!({"model":"m","messages":[{"role":"user","content":"Salut"}],"temperature":0.4,"top_p":0.8,"stop":["END"]});let request=simple_antigravity_chat(&chat).unwrap().1;assert_eq!(request["generationConfig"],json!({"temperature":0.4,"topP":0.8,"stopSequences":["END"]}));
        let response=json!({"model":"m","input":"Salut","temperature":0.4,"top_p":0.8});let request=simple_antigravity_responses(&response).unwrap().1;assert_eq!(request["generationConfig"]["temperature"],0.4);assert_eq!(request["generationConfig"]["topP"],0.8);
        let messages=json!({"model":"m","messages":[{"role":"user","content":"Salut"}],"max_tokens":50,"temperature":0.4,"top_p":0.8,"stop_sequences":["END"]});let request=simple_antigravity_messages(&messages).unwrap().1;assert_eq!(request["generationConfig"]["stopSequences"][0],"END");
        let tools=json!({"model":"m","messages":[{"role":"user","content":"Salut"}],"tools":[{"type":"function","function":{"name":"read"}}],"temperature":0.4,"top_p":0.8,"stop":"END"});let request=crate::antigravity_tools::prepare("s","r","a","m",&tools).unwrap();assert_eq!(request["generationConfig"],json!({"temperature":0.4,"topP":0.8,"stopSequences":["END"]}));
        for (key,invalid) in [("temperature",json!(2.1)),("temperature",json!(-1)),("top_p",json!(1.1)),("stop",json!([])),("stop",json!(["a","b","c","d","e"]))]{let mut bad=chat.clone();bad[key]=invalid;assert!(simple_antigravity_chat(&bad).is_err());}
    }
    #[test] fn antigravity_messages_validates_client_metadata(){
        let mut input=json!({"model":"m","messages":[{"role":"user","content":"Salut"}],"max_tokens":10,"metadata":{"user_id":"synthetic"}});
        let request=simple_antigravity_messages(&input).unwrap().1;assert!(request.get("metadata").is_none());
        input["metadata"]=json!("bad");assert!(simple_antigravity_messages(&input).is_err());
        input["metadata"]=json!({});input["output_config"]=json!({"effort":"high"});assert!(simple_antigravity_messages(&input).is_err());input["model"]=json!("gemini-3.1-pro-preview");assert_eq!(simple_antigravity_messages(&input).unwrap().1["generationConfig"]["thinkingConfig"]["thinkingLevel"],"HIGH");input["output_config"]=json!({"effort":"max"});assert!(simple_antigravity_messages(&input).is_err());
    }
    #[test] fn antigravity_output_limits_are_validated(){
        let mut chat=json!({"model":"m","messages":[{"role":"user","content":"Salut"}],"max_completion_tokens":42});
        assert_eq!(simple_antigravity_chat(&chat).unwrap().3,42);
        chat["max_tokens"]=json!(10);assert!(simple_antigravity_chat(&chat).is_err());
        for limit in [json!(0),json!(65537),json!(-1),json!("42"),json!(1.5)] {assert!(simple_antigravity_responses(&json!({"model":"m","input":"Salut","max_output_tokens":limit})).is_err());}
        assert_eq!(simple_antigravity_responses(&json!({"model":"m","input":"Salut","max_output_tokens":65536})).unwrap().3,65536);
    }
    #[test] fn antigravity_responses_only_accepts_supported_text_requests(){
        let input=json!({"model":"m","input":"Bonjour","stream":true,"store":false});assert_eq!(simple_antigravity_responses(&input).unwrap().1["contents"][0]["parts"][0]["text"],"Bonjour");
        assert!(simple_antigravity_responses(&json!({"model":"m","input":"Bonjour","tools":[]})).is_ok());
        assert!(simple_antigravity_responses(&json!({"model":"m","input":"Bonjour","tools":[{"type":"function"}]})).is_err());
        assert!(simple_antigravity_responses(&json!({"model":"m","input":"Bonjour","store":true})).is_err());
        let array=json!({"model":"m","input":[{"role":"user","type":"message","content":[{"type":"input_text","text":"Bonjour"}]}]});assert_eq!(simple_antigravity_responses(&array).unwrap().1["contents"][0]["parts"][0]["text"],"Bonjour");
        assert!(simple_antigravity_responses(&json!({"model":"m","input":[{"role":"user","content":[{"type":"input_image","image_url":"https://example.com"}]}]})).is_err());
        let history=json!({"model":"m","instructions":"Guide","input":[{"role":"user","content":[{"type":"input_text","text":"Salut"}]},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Bonjour"}]},{"role":"user","content":"Suite"}],"max_output_tokens":64});
        let (_,request,_,limit)=simple_antigravity_responses(&history).unwrap();assert_eq!(limit,64);assert_eq!(request["systemInstruction"]["parts"][0]["text"],"Guide");assert_eq!(request["contents"][1]["parts"][0]["text"],"Bonjour");assert_eq!(request["contents"][2]["parts"][0]["text"],"Suite");
        assert!(simple_antigravity_responses(&json!({"model":"m","input":[{"type":"function_call","call_id":"a"}]})).is_err());
        assert!(simple_antigravity_responses(&json!({"model":"m","input":[{"role":"user","content":[{"type":"output_text","text":"incorrect"}]}]})).is_err());

    }
    #[tokio::test] async fn antigravity_gateway_responses_stream_produces_completed_response(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-a".into(),"account-a".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|{let sender=sender.clone();forward_with_antigravity(request,router.clone(),move|_,_,_,request,_|{let sender=sender.clone();async move{if let Some(sender)=sender.lock().unwrap().take(){let _=sender.send(request);}Ok(crate::antigravity_login::GeneratedAnswer{text:"Bonjour".into(),input_tokens:Some(8),output_tokens:Some(3),truncated:false,reply:None})}},false)});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let input=json!({"model":"agy/m","instructions":"Guide","input":[{"role":"user","content":[{"type":"input_text","text":"Salut"}]},{"role":"assistant","content":[{"type":"output_text","text":"Bonjour"}]},{"role":"user","content":"Continue"}],"stream":true,"store":false,"tools":[]});let result=client.post(format!("http://{address}/v1/responses")).bearer_auth("key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);let body=result.text().await.unwrap();assert!(body.contains("response.completed"));assert!(body.contains("Bonjour"));assert!(body.contains("agy/m"));let request=rx.await.unwrap();assert_eq!(request["systemInstruction"]["parts"][0]["text"],"Guide");assert_eq!(request["contents"][1]["parts"][0]["text"],"Bonjour");assert_eq!(request["contents"][2]["parts"][0]["text"],"Continue");
    }
    #[tokio::test] async fn antigravity_gateway_http_routes_selected_account_and_refuses_unsupported_requests(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();
        let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-a".into(),"account-a".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"local-key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();let sender=sender.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|{let sender=sender.clone();forward_with_antigravity(request,router.clone(),move|reference,id,model,prompt,max_tokens|{let sender=sender.clone();async move{if let Some(sender)=sender.lock().unwrap().take(){let _=sender.send((reference,id,model,prompt,max_tokens));}Ok(crate::antigravity_login::GeneratedAnswer{text:"Réponse simulée".into(),input_tokens:Some(8),output_tokens:Some(3),truncated:false,reply:None})}},false)});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let input=json!({"model":"agy/model-a","messages":[{"role":"system","content":"Guide"},{"role":"user","content":"Bonjour"},{"role":"assistant","content":"Salut"},{"role":"user","content":"Continue"}],"max_tokens":123});
        let denied=client.post(format!("http://{address}/v1/chat/completions")).bearer_auth("wrong").body(input.to_string()).send().await.unwrap();assert_eq!(denied.status(),StatusCode::UNAUTHORIZED);
        let mut unsupported=input.clone();unsupported["tools"]=json!([{"type":"function"}]);let rejected=client.post(format!("http://{address}/v1/chat/completions")).bearer_auth("local-key").body(unsupported.to_string()).send().await.unwrap();assert_eq!(rejected.status(),StatusCode::BAD_REQUEST);
        let result=client.post(format!("http://{address}/v1/chat/completions")).bearer_auth("local-key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);
        let body:serde_json::Value=serde_json::from_slice(&result.bytes().await.unwrap()).unwrap();assert_eq!(body["model"],"agy/model-a");assert_eq!(body["choices"][0]["message"]["content"],"Réponse simulée");
        let (reference,account,model,prompt,limit)=rx.await.unwrap();assert_eq!((reference.as_str(),account.as_str(),model.as_str(),limit),("ref-a","account-a","model-a",123));assert_eq!(prompt["systemInstruction"]["parts"][0]["text"],"Guide");assert_eq!(prompt["contents"][0]["parts"][0]["text"],"Bonjour");assert_eq!(prompt["contents"][1]["parts"][0]["text"],"Salut");assert_eq!(prompt["contents"][2]["parts"][0]["text"],"Continue");
    }
    #[tokio::test] async fn antigravity_chat_tool_roundtrip_preserves_signed_google_parts(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-tools".into(),"account-tools".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"tool-local-key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();let sender=sender.clone();let count=count.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|{let sender=sender.clone();let count=count.clone();forward_with_antigravity(request,router.clone(),move|reference,account,model,prompt,_|{let sender=sender.clone();let count=count.clone();async move{
            assert_eq!((reference.as_str(),account.as_str(),model.as_str()),("ref-tools","account-tools","m"));
            if count.fetch_add(1,std::sync::atomic::Ordering::SeqCst)==0{
                assert_eq!(prompt["tools"][0]["functionDeclarations"][0]["name"],"read_file");
                let raw=json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read_file","args":{"path":"a.txt"}},"thoughtSignature":"exact-signature"}]},"finishReason":"STOP"}]});
                let reply=crate::google_request::response(&raw,"m").unwrap();Ok(crate::antigravity_login::GeneratedAnswer{text:String::new(),input_tokens:Some(2),output_tokens:Some(3),truncated:false,reply:Some(reply)})
            }else{if let Some(sender)=sender.lock().unwrap().take(){let _=sender.send(prompt);}Ok(crate::antigravity_login::GeneratedAnswer{text:"Contenu reçu".into(),input_tokens:Some(4),output_tokens:Some(3),truncated:false,reply:None})}
        }},false)});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let url=format!("http://{address}/v1/chat/completions");let tools=json!([{"type":"function","function":{"name":"read_file","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}}]);let mut input=json!({"model":"agy/m","messages":[{"role":"user","content":"Lis le fichier"}],"tools":tools});
        let response=client.post(&url).bearer_auth("tool-local-key").body(input.to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let body:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();assert_eq!(body["choices"][0]["finish_reason"],"tool_calls");let message=body["choices"][0]["message"].clone();let call_id=message["tool_calls"][0]["id"].as_str().unwrap().to_owned();
        let mut altered=message.clone();altered["tool_calls"][0]["function"]["arguments"]=json!("{}");let mut invalid=input.clone();invalid["messages"]=json!([{"role":"user","content":"Lis le fichier"},altered,{"role":"tool","tool_call_id":call_id,"content":"texte"}]);let rejected=client.post(&url).bearer_auth("tool-local-key").body(invalid.to_string()).send().await.unwrap();assert_eq!(rejected.status(),StatusCode::BAD_REQUEST);
        assert!(crate::antigravity_tools::prepare("tool-local-key","other-ref","account-tools","m",&invalid).is_err());
        let mut wrong_context=input.clone();wrong_context["messages"]=json!([{"role":"user","content":"Autre conversation"},message,{"role":"tool","tool_call_id":call_id,"content":"texte"}]);let rejected=client.post(&url).bearer_auth("tool-local-key").body(wrong_context.to_string()).send().await.unwrap();assert_eq!(rejected.status(),StatusCode::BAD_REQUEST);

        input["messages"]=json!([{"role":"user","content":"Lis le fichier"},message,{"role":"tool","tool_call_id":call_id,"content":"texte"}]);let response=client.post(&url).bearer_auth("tool-local-key").body(input.to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let body:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();assert_eq!(body["choices"][0]["message"]["content"],"Contenu reçu");let prompt=rx.await.unwrap();assert_eq!(prompt["contents"][1]["parts"][0]["thoughtSignature"],"exact-signature");assert_eq!(prompt["contents"][2]["parts"][0]["functionResponse"]["response"]["output"],"texte");assert!(prompt["contents"][2]["parts"][0]["functionResponse"].get("id").is_none());
    }
    #[tokio::test] async fn antigravity_responses_tool_roundtrip_preserves_signatures(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-responses".into(),"account-responses".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"responses-key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();let sender=sender.clone();let count=count.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|{let sender=sender.clone();let count=count.clone();forward_with_antigravity(request,router.clone(),move|_,_,_,prompt,_|{let sender=sender.clone();let count=count.clone();async move{if count.fetch_add(1,std::sync::atomic::Ordering::SeqCst)==0{assert_eq!(prompt["tools"][0]["functionDeclarations"][0]["name"],"read_file");let raw=json!({"candidates":[{"content":{"role":"model","parts":[{"text":"Je vérifie."},{"functionCall":{"name":"read_file","args":{"path":"a"}},"thoughtSignature":"sig"}]},"finishReason":"STOP"}]});let reply=crate::google_request::response(&raw,"m").unwrap();Ok(crate::antigravity_login::GeneratedAnswer{text:String::new(),input_tokens:Some(2),output_tokens:Some(3),truncated:false,reply:Some(reply)})}else{if let Some(sender)=sender.lock().unwrap().take(){let _=sender.send(prompt);}Ok(crate::antigravity_login::GeneratedAnswer{text:"Terminé".into(),input_tokens:Some(4),output_tokens:Some(3),truncated:false,reply:None})}}},false)});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let url=format!("http://{address}/v1/responses");let tools=json!([{"type":"function","name":"read_file","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]);let mut input=json!({"model":"agy/m","input":[{"role":"user","content":[{"type":"input_text","text":"Lis"},{"type":"input_image","image_url":"data:image/png;base64,AA=="}]}],"tools":tools,"store":false});
        let response=client.post(&url).bearer_auth("responses-key").body(input.to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let body:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();let call=body["output"][1].clone();assert_eq!(call["type"],"function_call");let id=call["call_id"].clone();let assistant=body["output"][0].clone();
        input["input"]=json!([{"role":"user","content":[{"type":"input_text","text":"Lis"},{"type":"input_image","image_url":"data:image/png;base64,AA=="}]},assistant,call,{"type":"function_call_output","call_id":id,"output":"texte"}]);let response=client.post(&url).bearer_auth("responses-key").body(input.to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let body:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();assert_eq!(body["output"][0]["content"][0]["text"],"Terminé");let prompt=rx.await.unwrap();assert_eq!(prompt["contents"][0]["parts"][1]["inlineData"]["data"],"AA==");assert_eq!(prompt["contents"][1]["parts"][1]["thoughtSignature"],"sig");assert_eq!(prompt["contents"][2]["parts"][0]["functionResponse"]["response"]["output"],"texte");
    }
    #[tokio::test] async fn antigravity_messages_tool_roundtrip_preserves_signatures(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-messages".into(),"account-messages".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"messages-key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();let sender=Arc::new(Mutex::new(Some(tx)));let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();let sender=sender.clone();let count=count.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|{let sender=sender.clone();let count=count.clone();forward_with_antigravity(request,router.clone(),move|_,_,_,prompt,_|{let sender=sender.clone();let count=count.clone();async move{if count.fetch_add(1,std::sync::atomic::Ordering::SeqCst)==0{assert_eq!(prompt["tools"][0]["functionDeclarations"][0]["name"],"read_file");let raw=json!({"candidates":[{"content":{"role":"model","parts":[{"text":"Je vérifie."},{"functionCall":{"name":"read_file","args":{"path":"a"}},"thoughtSignature":"sig-messages"}]},"finishReason":"STOP"}]});let reply=crate::google_request::response(&raw,"m").unwrap();Ok(crate::antigravity_login::GeneratedAnswer{text:String::new(),input_tokens:Some(2),output_tokens:Some(3),truncated:false,reply:Some(reply)})}else{if let Some(sender)=sender.lock().unwrap().take(){let _=sender.send(prompt);}Ok(crate::antigravity_login::GeneratedAnswer{text:"Terminé".into(),input_tokens:Some(4),output_tokens:Some(3),truncated:false,reply:None})}}},false)});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let url=format!("http://{address}/v1/messages");let tools=json!([{"name":"read_file","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}]);let mut input=json!({"model":"agy/m","messages":[{"role":"user","content":[{"type":"text","text":"Lis"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}],"tools":tools,"max_tokens":300});
        let response=client.post(&url).bearer_auth("messages-key").body(input.to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let body:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();assert_eq!(body["stop_reason"],"tool_use");assert_eq!(body["content"][0]["text"],"Je vérifie.");let call_id=body["content"][1]["id"].clone();let assistant=body["content"].clone();
        input["messages"]=json!([{"role":"user","content":[{"type":"text","text":"Lis"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]},{"role":"assistant","content":assistant},{"role":"user","content":[{"type":"tool_result","tool_use_id":call_id,"content":"texte"}]}]);let response=client.post(&url).bearer_auth("messages-key").body(input.to_string()).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);let body:serde_json::Value=serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();assert_eq!(body["content"][0]["text"],"Terminé");let prompt=rx.await.unwrap();assert_eq!(prompt["contents"][0]["parts"][1]["inlineData"]["data"],"AA==");assert_eq!(prompt["contents"][1]["parts"][1]["thoughtSignature"],"sig-messages");assert_eq!(prompt["contents"][2]["parts"][0]["functionResponse"]["response"]["output"],"texte");
    }
    #[tokio::test] async fn antigravity_tools_stream_in_all_client_formats(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-stream-tools".into(),"account-stream-tools".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"stream-tools-key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|forward_with_antigravity(request,router.clone(),|_,_,_,prompt,_|async move{assert_eq!(prompt["tools"][0]["functionDeclarations"][0]["name"],"read_file");if prompt["contents"].as_array().unwrap().len()>1{assert_eq!(prompt["contents"][1]["parts"][1]["thoughtSignature"],"stream-signature");assert_eq!(prompt["contents"][2]["parts"][0]["functionResponse"]["response"]["output"],"résultat");return Ok(crate::antigravity_login::GeneratedAnswer{text:"Terminé".into(),input_tokens:Some(9),output_tokens:Some(2),truncated:false,reply:None})}let raw=json!({"candidates":[{"content":{"role":"model","parts":[{"text":"Je vérifie."},{"functionCall":{"name":"read_file","args":{"path":"a"}},"thoughtSignature":"stream-signature"}]},"finishReason":"STOP"}]});let reply=crate::google_request::response(&raw,"m").unwrap();Ok(crate::antigravity_login::GeneratedAnswer{text:String::new(),input_tokens:Some(8),output_tokens:Some(3),truncated:false,reply:Some(reply)})},false));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let schema=json!({"type":"object","properties":{"path":{"type":"string"}}});
        for (path,mut input) in [
            ("chat/completions",json!({"model":"agy/m","messages":[{"role":"user","content":"Lis"}],"tools":[{"type":"function","function":{"name":"read_file","parameters":schema}}],"stream":true})),
            ("responses",json!({"model":"agy/m","input":"Lis","tools":[{"type":"function","name":"read_file","parameters":schema}],"stream":true,"store":false})),
            ("messages",json!({"model":"agy/m","messages":[{"role":"user","content":"Lis"}],"tools":[{"name":"read_file","input_schema":schema}],"stream":true,"max_tokens":100}))
        ]{
            let result=client.post(format!("http://{address}/v1/{path}")).bearer_auth("stream-tools-key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);assert_eq!(result.headers()["content-type"],"text/event-stream");let body=result.text().await.unwrap();assert!(body.contains("read_file"));assert!(body.contains("Je vérifie."));
            match path{"chat/completions"=>assert!(body.contains("\"finish_reason\":\"tool_calls\"")),"responses"=>assert!(body.contains("response.completed")),_=>{assert!(body.contains("event: message_stop"));assert!(body.contains("\"input_tokens\":8"));assert!(body.contains("\"output_tokens\":3"));}}
            let events:Vec<serde_json::Value>=body.lines().filter_map(|line|line.strip_prefix("data: ")).filter_map(|line|serde_json::from_str(line).ok()).collect();
            match path {
                "chat/completions"=>{let call=events.iter().find_map(|v|v.pointer("/choices/0/delta/tool_calls/0")).unwrap();let id=call["id"].clone();let function=call["function"].clone();input["messages"]=json!([{"role":"user","content":"Lis"},{"role":"assistant","content":"Je vérifie.","tool_calls":[{"id":id,"type":"function","function":function}]},{"role":"tool","tool_call_id":id,"content":"résultat"}]);},
                "responses"=>{let output=events.iter().find(|v|v["type"]=="response.completed").unwrap()["response"]["output"].as_array().unwrap();let id=output[1]["call_id"].clone();input["input"]=json!([{"role":"user","content":"Lis"},output[0],output[1],{"type":"function_call_output","call_id":id,"output":"résultat"}]);},
                _=>{let call=events.iter().find(|v|v["type"]=="content_block_start"&&v["content_block"]["type"]=="tool_use").unwrap();let id=call["content_block"]["id"].clone();let arguments=events.iter().find(|v|v["type"]=="content_block_delta"&&v["delta"]["type"]=="input_json_delta").unwrap()["delta"]["partial_json"].as_str().unwrap();let args:serde_json::Value=serde_json::from_str(arguments).unwrap();input["messages"]=json!([{"role":"user","content":"Lis"},{"role":"assistant","content":[{"type":"text","text":"Je vérifie."},{"type":"tool_use","id":id,"name":"read_file","input":args}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"résultat"}]}]);}
            }
            let result=client.post(format!("http://{address}/v1/{path}")).bearer_auth("stream-tools-key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);assert!(result.text().await.unwrap().contains("Terminé"));
        }
    }
    #[tokio::test] async fn antigravity_gateway_messages_text_and_usage(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();
        let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-a".into(),"account-a".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let counter=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();let counter=counter.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|{let counter=counter.clone();forward_with_antigravity(request,router.clone(),move|reference,id,model,request,limit|{let counter=counter.clone();async move{assert_eq!((reference.as_str(),id.as_str(),model.as_str(),limit),("ref-a","account-a","m",100));assert_eq!(request["systemInstruction"]["parts"][0]["text"],"Guide");assert_eq!(request["contents"][2]["parts"][0]["text"],"Suite");let usage=counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst)<2;Ok(crate::antigravity_login::GeneratedAnswer{text:"Bonjour".into(),input_tokens:usage.then_some(8),output_tokens:usage.then_some(3),truncated:false,reply:None})}},false)});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let mut input=json!({"model":"agy/m","system":"Guide","messages":[{"role":"user","content":"Salut"},{"role":"assistant","content":[{"type":"text","text":"Bonjour"}]},{"role":"user","content":"Suite"}],"max_tokens":100});
        let url=format!("http://{address}/v1/messages");
        let result=client.post(&url).bearer_auth("key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);let body:serde_json::Value=serde_json::from_slice(&result.bytes().await.unwrap()).unwrap();assert_eq!(body["content"][0]["text"],"Bonjour");assert_eq!(body["usage"]["input_tokens"],8);assert_eq!(body["usage"]["output_tokens"],3);
        input["stream"]=json!(true);let result=client.post(&url).bearer_auth("key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);let body=result.text().await.unwrap();assert!(body.contains("event: message_stop"));assert!(body.contains("\"input_tokens\":8"));assert!(body.contains("\"output_tokens\":3"));
        let result=client.post(&url).bearer_auth("key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::BAD_GATEWAY);
        input["tools"]=json!([{"name":"x"}]);let result=client.post(&url).bearer_auth("key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::BAD_REQUEST);
    }
    #[tokio::test] async fn antigravity_gateway_preserves_partial_answers_in_all_protocols(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref".into(),"id".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move|request|forward_with_antigravity(request,router.clone(),|_,_,_,_,_|async{Ok(crate::antigravity_login::GeneratedAnswer{text:"partiel".into(),input_tokens:Some(8),output_tokens:Some(3),truncated:true,reply:None})},false));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        for (path,mut input) in [
            ("chat/completions",json!({"model":"agy/m","messages":[{"role":"user","content":"Salut"}]})),
            ("responses",json!({"model":"agy/m","input":"Salut"})),
            ("messages",json!({"model":"agy/m","messages":[{"role":"user","content":"Salut"}],"max_tokens":40}))
        ]{
            for streaming in [false,true]{
                input["stream"]=json!(streaming);
                let result=client.post(format!("http://{address}/v1/{path}")).bearer_auth("key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);
                let body=result.text().await.unwrap();assert!(body.contains("partiel"));
                if streaming {
                    match path{"chat/completions"=>assert!(body.contains("\"finish_reason\":\"length\"")),"responses"=>assert!(body.contains("response.incomplete")),_=>assert!(body.contains("\"stop_reason\":\"max_tokens\""))}
                }else{
                    let json:serde_json::Value=serde_json::from_str(&body).unwrap();
                    match path{"chat/completions"=>{assert_eq!(json["choices"][0]["finish_reason"],"length");assert_eq!(json["usage"]["prompt_tokens"],8)},"responses"=>{assert_eq!(json["status"],"incomplete");assert_eq!(json["incomplete_details"]["reason"],"max_output_tokens");assert_eq!(json["usage"]["input_tokens"],8)},_=>assert_eq!(json["stop_reason"],"max_tokens")}
                }
            }
        }
    }
    #[tokio::test] async fn antigravity_gateway_returns_buffered_chat_sse(){
        let client=reqwest::Client::builder().no_proxy().build().unwrap();let upstream=Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:client.clone(),chat_only:true,anthropic_native:false,antigravity:Some(("ref-a".into(),"account-a".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None});
        let router=Arc::new(Router{token:"key".into(),default_id:"agy".into(),providers:RwLock::new(BTreeMap::from([("agy".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let service=service_fn(move|request|forward_with_antigravity(request,router.clone(),|_,_,_,_,_|async{Ok(crate::antigravity_login::GeneratedAnswer{text:"Bonjour".into(),input_tokens:Some(8),output_tokens:Some(3),truncated:false,reply:None})},false));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let input=json!({"model":"agy/m","messages":[{"role":"user","content":"Salut"}],"stream":true});let result=client.post(format!("http://{address}/v1/chat/completions")).bearer_auth("key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);assert_eq!(result.headers()["content-type"],"text/event-stream");let body=result.text().await.unwrap();let events:Vec<serde_json::Value>=body.lines().filter_map(|line|line.strip_prefix("data: ")).filter(|line|*line!="[DONE]").map(|line|serde_json::from_str(line).unwrap()).collect();assert_eq!(events.len(),3);assert_eq!(events[1]["choices"][0]["delta"]["content"],"Bonjour");assert_eq!(events[2]["choices"][0]["finish_reason"],"stop");assert!(body.contains("data: [DONE]"));
    }
    #[test]
    fn deepseek_balance_preserves_amounts_and_restricts_destination(){
        for url in ["https://api.deepseek.com","https://api.deepseek.com/v1"]{assert!(is_official_deepseek(&create_upstream(url,"x".into(),true).unwrap()))}
        for url in ["https://api.deepseek.com.evil.example/v1","https://api.deepseek.com/other","http://127.0.0.1:1234/v1"]{assert!(!is_official_deepseek(&create_upstream(url,"x".into(),true).unwrap()))}
        let data=json!({"is_available":true,"balance_infos":[{"currency":"USD","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"},{"currency":"USD","total_balance":"oops"}]});
        let balance=parse_deepseek_balance(&data).unwrap();assert!(balance.available);assert_eq!(balance.balances.len(),1);assert_eq!(balance.balances[0].total,"110.00");
        assert!(parse_deepseek_balance(&json!({"is_available":true,"balance_infos":[{"currency":"USD","total_balance":"NaN"}]})).is_err());
    }
    #[tokio::test]
    async fn deepseek_balance_reads_simulated_service(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move|request:Request<Incoming>|{let tx=tx.clone();async move{let path=request.uri().path().to_string();let auth=request.headers()["authorization"].to_str().unwrap().to_owned();tx.lock().unwrap().take().unwrap().send((path,auth)).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"is_available":true,"balance_infos":[{"currency":"USD","total_balance":"3.20"}]}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=create_upstream(&format!("http://{address}/v1"),"synthetic".into(),true).unwrap();let url=format!("http://{address}/user/balance").parse().unwrap();let balance=fetch_deepseek_balance(upstream,url).await.unwrap();
        assert_eq!(balance.balances[0].total,"3.20");assert_eq!(rx.await.unwrap(),("/user/balance".into(),"Bearer synthetic".into()));
    }
    #[test]
    fn openrouter_usage_uses_only_official_endpoint_and_valid_numbers(){
        let official=create_upstream("https://openrouter.ai/api/v1","secret".into(),true).unwrap();assert!(is_official_openrouter(&official));
        for url in ["https://openrouter.ai.evil.example/api/v1","https://openrouter.ai/other","http://127.0.0.1:1234/api/v1"]{let upstream=create_upstream(url,"secret".into(),true).unwrap();assert!(!is_official_openrouter(&upstream));}
        let result=parse_openrouter_usage(&json!({"data":{"limit":100.0,"limit_remaining":0,"usage":25.5,"usage_daily":-1,"limit_reset":"monthly","label":"do-not-return"}})).unwrap();
        assert_eq!(result.limit,Some(100.0));assert_eq!(result.remaining,Some(0.0));assert_eq!(result.daily,None);assert_eq!(result.reset.as_deref(),Some("monthly"));
        assert!(!serde_json::to_string(&result).unwrap().contains("do-not-return"));
        assert!(parse_openrouter_usage(&json!({"data":null})).is_err());
    }
    #[tokio::test]
    async fn openrouter_usage_parses_key_response_from_simulated_service(){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{let(socket,_)=listener.accept().await.unwrap();let tx=Arc::new(Mutex::new(Some(tx)));let service=service_fn(move |request:Request<Incoming>|{let tx=tx.clone();async move{let path=request.uri().path().to_string();let auth=request.headers()["authorization"].to_str().unwrap().to_owned();tx.lock().unwrap().take().unwrap().send((path,auth)).unwrap();Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"data":{"limit":20,"limit_remaining":7,"usage":13}}"#))))}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let upstream=create_upstream(&format!("http://{address}/api/v1"),"synthetic".into(),true).unwrap();let url=upstream.base.join("key").unwrap();let result=fetch_openrouter_usage(upstream,url).await.unwrap();
        assert_eq!(result.remaining,Some(7.0));assert_eq!(rx.await.unwrap(),("/api/v1/key".into(),"Bearer synthetic".into()));
    }
    #[test]
    fn codex_launch_requires_an_active_explicit_provider_for_nested_models() {
        let first=create_upstream("https://example.com/v1", "first".into(), false).unwrap();
        let router=Router{token:"test".into(),default_id:"first".into(),providers:RwLock::new(BTreeMap::from([("first".into(),first)]))};
        assert_eq!(launch_provider(&router,"plain").unwrap().key,"first");
        assert_eq!(launch_provider(&router,"first/vendor/model").unwrap().key,"first");
        for model in ["removed/model","first/","-bad","bad model",""] { assert!(launch_provider(&router,model).is_err()); }
    }
    #[cfg(windows)]
    #[test]
    fn codex_launch_keeps_user_values_and_token_out_of_the_command_line() {
        let secret="synthetic-secret-for-test";
        let model="first/model;echo-danger";
        let command=codex_launch_command(std::path::Path::new("C:\\codex.exe"),std::path::Path::new("C:\\project"),model,"http://127.0.0.1:1234/v1",secret,true).unwrap();
        let args:Vec<_>=command.get_args().map(|value|value.to_string_lossy().into_owned()).collect();
        assert!(!args.iter().any(|arg|arg.contains(secret)||arg.contains(model)));
        let envs:std::collections::HashMap<_,_>=command.get_envs().map(|(key,value)|(key.to_string_lossy().into_owned(),value.unwrap().to_string_lossy().into_owned())).collect();
        assert_eq!(envs.get("NIVRA_GATEWAY_KEY").unwrap(),secret);
        assert_eq!(envs.get("NIVRA_CODEX_MODEL").unwrap(),model);
        assert_eq!(envs.get("NIVRA_CODEX_NO_WEB").unwrap(),"1");
    }
    #[cfg(windows)]
    #[test]
    fn claude_launch_uses_local_gateway_and_keeps_its_token_out_of_arguments() {
        let command=claude_launch_command(std::path::Path::new("C:\\claude.exe"),std::path::Path::new("C:\\project"),"first/vendor/model","http://127.0.0.1:1234/v1","synthetic-secret").unwrap();
        let args:Vec<_>=command.get_args().map(|v|v.to_string_lossy().into_owned()).collect();
        assert_eq!(args,["--model","first/vendor/model"]);
        let envs:std::collections::HashMap<_,_>=command.get_envs().map(|(key,value)|(key.to_string_lossy().into_owned(),value.map(|v|v.to_string_lossy().into_owned()))).collect();
        assert_eq!(envs["ANTHROPIC_BASE_URL"].as_deref(),Some("http://127.0.0.1:1234"));
        assert_eq!(envs["ANTHROPIC_API_KEY"].as_deref(),Some("synthetic-secret"));
        assert_eq!(envs["ANTHROPIC_AUTH_TOKEN"],None);
        assert_eq!(envs["CLAUDE_CODE_OAUTH_TOKEN"],None);
    }
    #[test]
    fn validates_upstream_addresses() {
        assert_eq!(upstream_url("https://example.com/v1").unwrap().as_str(), "https://example.com/v1/");
        assert!(upstream_url("http://127.0.0.1:11434/v1").is_ok());
        assert!(upstream_url("http://example.com/v1").is_err());
        assert!(upstream_url("https://user:secret@example.com/v1").is_err());
        assert!(upstream_url("https://example.com/v1?key=secret").is_err());
        assert!(valid_id("open-router_2"));
        assert!(!valid_id("Open/router"));
        assert!(safe_model_query("limit=100&after_id=claude-model-1"));
        assert!(!safe_model_query("limit=1001"));
        assert!(!safe_model_query("limit=1&limit=2"));
        assert!(!safe_model_query("after_id=x&before_id=y"));
        assert!(!safe_model_query("api_key=not-allowed"));
    }
    #[test]
    fn routes_only_registered_prefixes_and_preserves_nested_model_names() {
        let first = create_upstream("https://example.com/v1", "first".into(), false).unwrap();
        let second = create_upstream("https://example.org/v1", "second".into(), false).unwrap();
        let router = Router { token: "test".into(), default_id: "first".into(), providers: RwLock::new(BTreeMap::from([("first".into(), first), ("second".into(), second)])) };
        let (chosen, body) = select_provider(&router, &Bytes::from_static(br#"{"model":"second/vendor/model","stream":true}"#)).unwrap();
        assert_eq!(chosen.key, "second");
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["model"], "vendor/model");
        let original = Bytes::from_static(br#"{"model":"plain-model"}"#);
        let (chosen, body) = select_provider(&router, &original).unwrap();
        assert_eq!(chosen.key, "first");
        assert_eq!(body, original);
        assert!(select_provider(&router, &Bytes::from_static(br#"{"model":"vendor/model"}"#)).is_err());
        assert!(select_provider(&router, &Bytes::from_static(br#"{"messages":[]}"#)).is_err());
    }

    #[test]
    fn removed_provider_never_falls_back_to_default() {
        let first=create_upstream("https://example.com/v1","first".into(),false).unwrap();
        let router=Router{token:"test".into(),default_id:"first".into(),providers:RwLock::new(BTreeMap::from([("first".into(),first)]))};
        assert!(select_provider(&router,&Bytes::from_static(br#"{"model":"removed/model"}"#)).is_err());
        assert!(select_provider(&router,&Bytes::from_static(br#"{"model":"first/"}"#)).is_err());
        let (_,body)=select_provider(&router,&Bytes::from_static(br#"{"model":"first/vendor/model"}"#)).unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["model"],"vendor/model");
    }
    #[test]
    fn anthropic_chat_rejects_unrepresentable_options(){
        let mut value=json!({"model":"claude","messages":[{"role":"user","content":"Bonjour"}],"max_completion_tokens":42,"stream":true,"stream_options":{"include_usage":true},"n":1});
        let (converted,usage)=anthropic_chat_request(&value).unwrap();assert!(usage);assert_eq!(converted["max_tokens"],42);
        value["n"]=json!(2);assert!(anthropic_chat_request(&value).is_err());
        value.as_object_mut().unwrap().remove("n");value["max_completion_tokens"]=json!("invalid");assert!(anthropic_chat_request(&value).is_err());
    }
    #[test]
    fn anthropic_chat_stream_preserves_chat_metadata_and_usage(){
        let mut stream=AnthropicChatStream::new("anthropic/m".into(),true);
        let initial=stream.start();assert!(String::from_utf8_lossy(&initial[0]).contains("assistant"));
        let events=[json!({"type":"message_start","message":{"usage":{"input_tokens":3}}}),json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Salut"}}),json!({"type":"content_block_stop","index":0}),json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),json!({"type":"message_stop"})];
        let input=events.iter().map(|event|format!("data: {event}\n\n")).collect::<String>();
        let output=stream.ingest(input.as_bytes()).unwrap();assert!(stream.bridge.is_done());
        let joined=output.iter().map(|part|String::from_utf8_lossy(part)).collect::<String>();
        assert!(joined.contains("chat.completion.chunk"));assert!(joined.contains("anthropic/m"));assert!(joined.contains("Salut"));assert!(joined.contains("\"total_tokens\":5"));assert!(joined.contains("[DONE]"));
    }

    #[tokio::test]
    async fn combines_available_catalogs_with_unambiguous_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let tx = Arc::new(Mutex::new(Some(tx)));
            let service = service_fn(move |request: Request<Incoming>| {
                let tx = tx.clone();
                async move {
                    let auth = request.headers().get("authorization").unwrap().to_str().unwrap().to_string();
                    tx.lock().unwrap().take().unwrap().send((request.uri().path().to_string(), auth)).unwrap();
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"data":[{"id":"plain","object":"model"},{"id":"vendor/nested","object":"model"}]}"#))))
                }
            });
            let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).await;
        });
        let available = create_upstream(&format!("http://{address}/v1"), "key-one".into(), false).unwrap();
        let unavailable = create_upstream("http://127.0.0.1:1/v1", "key-two".into(), false).unwrap();
        let router = Router { token:"test".into(), default_id:"one".into(), providers:RwLock::new(BTreeMap::from([("one".into(), available), ("two".into(), unavailable)])) };
        let result = combined_catalog(&router).await;
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(result.headers().get("x-nivra-catalog-incomplete").unwrap(), "true");
        let json: serde_json::Value = serde_json::from_slice(&result.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let ids: Vec<_> = json["data"].as_array().unwrap().iter().map(|entry| entry["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["plain", "one/plain", "one/vendor/nested"]);
        assert_eq!(rx.await.unwrap(), ("/v1/models".into(), "Bearer key-one".into()));
    }

    #[tokio::test]
    async fn single_provider_catalog_exposes_nested_models_with_routable_prefix() {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap();
            let service=service_fn(|_request:Request<Incoming>|async move {
                Ok::<_,Infallible>(Response::new(Full::new(Bytes::from_static(br#"{"data":[{"id":"plain"},{"id":"vendor/nested"}]}"#))))
            });
            let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=create_upstream(&format!("http://{address}/v1"),"key".into(),false).unwrap();
        let router=Router{token:"test".into(),default_id:"one".into(),providers:RwLock::new(BTreeMap::from([("one".into(),upstream)]))};
        let result=combined_catalog(&router).await;
        assert_eq!(result.status(),StatusCode::OK);
        let value:serde_json::Value=serde_json::from_slice(&result.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let ids:Vec<_>=value["data"].as_array().unwrap().iter().map(|entry|entry["id"].as_str().unwrap()).collect();
        assert_eq!(ids,["plain","one/plain","one/vendor/nested"]);
        for id in ids {let body=Bytes::from(serde_json::json!({"model":id}).to_string());assert!(select_provider(&router,&body).is_ok());}
    }

    #[tokio::test]
    async fn translates_responses_to_chat_over_http() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let (socket, _) = upstream_listener.accept().await.unwrap();
            let sender = Arc::new(Mutex::new(Some(observed_tx)));
            let service = service_fn(move |request: Request<Incoming>| {
                let sender=sender.clone();
                async move {
                    let path=request.uri().path().to_string();
                    let data=request.into_body().collect().await.unwrap().to_bytes();
                    sender.lock().unwrap().take().unwrap().send((path,data)).unwrap();
                    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                    Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from_static(body))).unwrap())
                }
            });
            let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=create_upstream(&format!("http://{upstream_address}/v1"),"key".into(),true).unwrap();
        let router=Arc::new(Router{token:"gateway-secret".into(),default_id:"provider".into(),providers:RwLock::new(BTreeMap::from([("provider".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap();
            let service=service_fn(move |request|forward(request,router.clone()));
            let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let result=reqwest::Client::new().post(format!("http://{address}/v1/responses")).bearer_auth("gateway-secret").header("content-type","application/json").body(serde_json::json!({"model":"provider/test","input":"Hi","stream":true}).to_string()).send().await.unwrap();
        assert_eq!(result.status(),StatusCode::OK);
        let text=result.text().await.unwrap();
        assert!(text.contains("event: response.output_text.delta"));
        assert!(text.contains("event: response.completed"));
        assert!(text.contains("provider/test"));
        let (path,body)=observed_rx.await.unwrap();
        let json:serde_json::Value=serde_json::from_slice(&body).unwrap();
        assert_eq!(path,"/v1/chat/completions");
        assert_eq!(json["model"],"test");
        assert_eq!(json["messages"][0]["content"],"Hi");
    }

    #[tokio::test]
    async fn translates_messages_to_chat_with_anthropic_auth() {
        let mock=TcpListener::bind("127.0.0.1:0").await.unwrap();let mock_address=mock.local_addr().unwrap();
        let (tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let (socket,_)=mock.accept().await.unwrap();let tx=Arc::new(Mutex::new(Some(tx)));
            let service=service_fn(move |request:Request<Incoming>|{let tx=tx.clone();async move{
                let path=request.uri().path().to_string();let auth=request.headers().get("authorization").unwrap().to_str().unwrap().to_string();
                let body=request.into_body().collect().await.unwrap().to_bytes();tx.lock().unwrap().take().unwrap().send((path,auth,body)).unwrap();
                let body=b"data: {\"choices\":[{\"delta\":{\"content\":\"Bonjour\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from_static(body))).unwrap())
            }});
            let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=create_upstream(&format!("http://{mock_address}/v1"),"provider-key".into(),true).unwrap();
        let router=Arc::new(Router{token:"gateway-key".into(),default_id:"test".into(),providers:RwLock::new(BTreeMap::from([("test".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {let (socket,_)=listener.accept().await.unwrap();let service=service_fn(move |request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let body=json!({"model":"test/model","max_tokens":100,"messages":[{"role":"user","content":"Salut"}],"stream":true});
        let result=reqwest::Client::new().post(format!("http://{address}/v1/messages?beta=true")).header("x-api-key","gateway-key").header("anthropic-version","2023-06-01").body(body.to_string()).send().await.unwrap();
        assert_eq!(result.status(),StatusCode::OK);let result=result.text().await.unwrap();
        assert!(result.contains("event: message_start"));assert!(result.contains("event: content_block_delta"));assert!(result.contains("event: message_stop"));
        let(path,auth,body)=rx.await.unwrap();let body:serde_json::Value=serde_json::from_slice(&body).unwrap();
        assert_eq!(path,"/v1/chat/completions");assert_eq!(auth,"Bearer provider-key");assert_eq!(body["model"],"model");assert_eq!(body["messages"][0]["content"],"Salut");
    }

    #[tokio::test]
    async fn forwards_native_anthropic_messages_and_token_count() {
        let mock=TcpListener::bind("127.0.0.1:0").await.unwrap();let mock_address=mock.local_addr().unwrap();
        let(tx,mut rx)=tokio::sync::mpsc::unbounded_channel();
        tauri::async_runtime::spawn(async move{loop{
            let Ok((socket,_))=mock.accept().await else{break};let tx=tx.clone();
            tauri::async_runtime::spawn(async move{let service=service_fn(move |request:Request<Incoming>|{let tx=tx.clone();async move{
                let path=request.uri().to_string();
                let key=request.headers().get("x-api-key").unwrap().to_str().unwrap().to_string();
                let version=request.headers().get("anthropic-version").unwrap().to_str().unwrap().to_string();
                let beta=request.headers().get("anthropic-beta").unwrap().to_str().unwrap().to_string();
                let leaked_auth=request.headers().contains_key("authorization");
                let body=request.into_body().collect().await.unwrap().to_bytes();let _=tx.send((path,key,version,beta,leaked_auth,body));
                Ok::<_,Infallible>(Response::builder().header("content-type","application/json").body(Full::new(Bytes::from_static(br#"{"input_tokens":42}"#))).unwrap())
            }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        }});
        let upstream=create_upstream_with_mode(&format!("http://{mock_address}/v1"),"provider-secret".into(),false,true).unwrap();
        let router=Arc::new(Router{token:"gateway-secret".into(),default_id:"anthropic".into(),providers:RwLock::new(BTreeMap::from([("anthropic".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move |request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let client=reqwest::Client::new();
        for endpoint in ["messages?beta=true","messages/count_tokens"] {
            let input=json!({"model":"anthropic/example","thinking":{"type":"enabled","budget_tokens":1024},"messages":[{"role":"user","content":"Hi"}]});
            let result=client.post(format!("http://{address}/v1/{endpoint}")).header("x-api-key","gateway-secret").header("anthropic-version","2023-06-01").header("anthropic-beta","example-beta").body(input.to_string()).send().await.unwrap();
            assert_eq!(result.status(),StatusCode::OK);
            let(path,key,version,beta,leaked_auth,body)=tokio::time::timeout(Duration::from_secs(3),rx.recv()).await.unwrap().unwrap();
            assert_eq!(path,format!("/v1/{endpoint}"));assert_eq!(key,"provider-secret");assert_eq!(version,"2023-06-01");assert_eq!(beta,"example-beta");assert!(!leaked_auth);
            let value:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(value["model"],"example");assert_eq!(value["thinking"]["type"],"enabled");
        }
    }

    #[tokio::test]
    async fn routes_responses_tools_through_anthropic_stream() {
        let mock=TcpListener::bind("127.0.0.1:0").await.unwrap();let mock_address=mock.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{
            let Ok((socket,_))=mock.accept().await else{return};let tx=Arc::new(Mutex::new(Some(tx)));
            let service=service_fn(move |request:Request<Incoming>|{let tx=tx.clone();async move{
                let path=request.uri().path().to_string();let key=request.headers().get("x-api-key").unwrap().to_str().unwrap().to_string();
                let body=request.into_body().collect().await.unwrap().to_bytes();if let Some(tx)=tx.lock().unwrap().take(){let _=tx.send((path,key,body));}
                let data=b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"work__read\",\"input\":{}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"a\\\"}\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":2}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
                Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from_static(data))).unwrap())
            }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=create_upstream_with_mode(&format!("http://{mock_address}/v1"),"provider-key".into(),false,true).unwrap();
        let router=Arc::new(Router{token:"gateway-key".into(),default_id:"anthropic".into(),providers:RwLock::new(BTreeMap::from([("anthropic".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{let Ok((socket,_))=listener.accept().await else{return};let service=service_fn(move |request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let input=json!({"model":"anthropic/test","input":"Read","stream":true,"tools":[{"type":"namespace","name":"work","tools":[{"type":"function","name":"read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]}]});
        let result=reqwest::Client::new().post(format!("http://{address}/v1/responses")).bearer_auth("gateway-key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);
        let body=result.text().await.unwrap();let events:Vec<serde_json::Value>=body.lines().filter_map(|line|line.strip_prefix("data: ")).map(|line|serde_json::from_str(line).unwrap()).collect();
        let final_event=events.iter().find(|event|event["type"]=="response.completed").unwrap();assert_eq!(final_event["response"]["output"][0]["namespace"],"work");assert_eq!(final_event["response"]["output"][0]["arguments"],"{\"path\":\"a\"}");
        let(path,key,body)=rx.await.unwrap();let body:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(path,"/v1/messages");assert_eq!(key,"provider-key");assert_eq!(body["tools"][0]["name"],"work__read");
    }

    #[tokio::test]
    async fn routes_chat_to_anthropic_and_returns_chat_completion() {
        let mock=TcpListener::bind("127.0.0.1:0").await.unwrap();let mock_address=mock.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{
            let (socket,_)=mock.accept().await.unwrap();let tx=Arc::new(Mutex::new(Some(tx)));
            let service=service_fn(move |request:Request<Incoming>|{let tx=tx.clone();async move{
                let path=request.uri().path().to_string();let key=request.headers()["x-api-key"].to_str().unwrap().to_string();let body=request.into_body().collect().await.unwrap().to_bytes();
                tx.lock().unwrap().take().unwrap().send((path,key,body)).unwrap();
                let body=json!({"content":[{"type":"text","text":"Bonjour"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}).to_string();
                Ok::<_,Infallible>(Response::builder().header("content-type","application/json").body(Full::new(Bytes::from(body))).unwrap())
            }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=create_upstream_with_mode(&format!("http://{mock_address}/v1"),"provider-key".into(),false,true).unwrap();
        let router=Arc::new(Router{token:"gateway-key".into(),default_id:"anthropic".into(),providers:RwLock::new(BTreeMap::from([("anthropic".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{let (socket,_)=listener.accept().await.unwrap();let service=service_fn(move |request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let input=json!({"model":"anthropic/test","messages":[{"role":"user","content":"Salut"}],"max_completion_tokens":40});
        let result=reqwest::Client::new().post(format!("http://{address}/v1/chat/completions")).bearer_auth("gateway-key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);
        let body:serde_json::Value=serde_json::from_slice(&result.bytes().await.unwrap()).unwrap();assert_eq!(body["object"],"chat.completion");assert_eq!(body["model"],"anthropic/test");assert_eq!(body["choices"][0]["message"]["content"],"Bonjour");
        let (path,key,body)=rx.await.unwrap();let body:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(path,"/v1/messages");assert_eq!(key,"provider-key");assert_eq!(body["model"],"test");assert_eq!(body["max_tokens"],40);
    }

    #[tokio::test]
    async fn streams_chat_tool_calls_from_anthropic_over_http(){
        let mock=TcpListener::bind("127.0.0.1:0").await.unwrap();let mock_address=mock.local_addr().unwrap();let(tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{
            let (socket,_)=mock.accept().await.unwrap();let tx=Arc::new(Mutex::new(Some(tx)));
            let service=service_fn(move |request:Request<Incoming>|{let tx=tx.clone();async move{
                let path=request.uri().path().to_string();let body=request.into_body().collect().await.unwrap().to_bytes();tx.lock().unwrap().take().unwrap().send((path,body)).unwrap();
                let events=[json!({"type":"message_start","message":{"usage":{"input_tokens":3}}}),json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_1","name":"read","input":{}}}),json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}),json!({"type":"content_block_stop","index":0}),json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),json!({"type":"message_stop"})];
                let encoded=events.iter().map(|event|format!("data: {event}\n\n")).collect::<String>();
                Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from(encoded))).unwrap())
            }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=create_upstream_with_mode(&format!("http://{mock_address}/v1"),"provider-key".into(),false,true).unwrap();
        let router=Arc::new(Router{token:"gateway-key".into(),default_id:"anthropic".into(),providers:RwLock::new(BTreeMap::from([("anthropic".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{let (socket,_)=listener.accept().await.unwrap();let service=service_fn(move |request|forward(request,router.clone()));let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});
        let input=json!({"model":"anthropic/test","messages":[{"role":"user","content":"Read"}],"stream":true,"stream_options":{"include_usage":true},"tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}}]});
        let result=reqwest::Client::new().post(format!("http://{address}/v1/chat/completions")).bearer_auth("gateway-key").body(input.to_string()).send().await.unwrap();assert_eq!(result.status(),StatusCode::OK);
        let result=result.text().await.unwrap();let events:Vec<serde_json::Value>=result.lines().filter_map(|line|line.strip_prefix("data: ")).filter(|line|*line!="[DONE]").map(|line|serde_json::from_str(line).unwrap()).collect();
        assert!(result.contains("data: [DONE]"));assert!(events.iter().all(|event|event["model"]=="anthropic/test"&&event["object"]=="chat.completion.chunk"));
        assert!(events.iter().any(|event|event["choices"][0]["delta"]["tool_calls"][0]["function"]["name"]=="read"));
        assert!(events.iter().any(|event|event["choices"][0]["finish_reason"]=="tool_calls"));assert!(events.iter().any(|event|event["usage"]["total_tokens"]==5));
        let(path,body)=rx.await.unwrap();let body:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(path,"/v1/messages");assert_eq!(body["model"],"test");assert_eq!(body["tools"][0]["name"],"read");assert_eq!(body["stream"],true);
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Runs Claude Code in bare mode against a local simulated provider"]
    async fn claude_cli_smoke_with_isolated_home() {
        use std::{process::{Command,Stdio},io::Read};
        let executable=std::env::var_os("NIVRA_CLAUDE_EXECUTABLE").expect("Set NIVRA_CLAUDE_EXECUTABLE to the Claude Code binary");
        let native=std::env::var_os("NIVRA_CLAUDE_NATIVE").is_some();let antigravity=std::env::var_os("NIVRA_CLAUDE_ANTIGRAVITY").is_some();let test_tool=std::env::var_os("NIVRA_CLAUDE_ANTIGRAVITY_TOOL").is_some();
        let temporary=tempfile::Builder::new().prefix("nivra-claude-smoke-").tempdir_in("target").unwrap();
        let root=temporary.path().canonicalize().unwrap();let config=root.join("claude-config");std::fs::create_dir_all(&config).unwrap();let test_file=root.join("marker.txt");std::fs::write(&test_file,"NIVRA_TOOL_OK").unwrap();let test_file=test_file.to_string_lossy().to_string();
        let mock=TcpListener::bind("127.0.0.1:0").await.unwrap();let mock_address=mock.local_addr().unwrap();
        let (tx,rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move{
            let Ok((socket,_))=mock.accept().await else{return};let tx=Arc::new(Mutex::new(Some(tx)));
            let service=service_fn(move |request:Request<Incoming>|{let tx=tx.clone();async move{
                let path=request.uri().path().to_string();let body=request.into_body().collect().await.unwrap().to_bytes();
                if let Some(tx)=tx.lock().unwrap().take(){let _=tx.send((path,body));}
                let data: &'static [u8]=if native { b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"test-model\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"NIVRA_CLAUDE_OK\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n" } else { b"data: {\"choices\":[{\"delta\":{\"content\":\"NIVRA_CLAUDE_OK\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n" };
                Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from_static(data))).unwrap())
            }});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=if antigravity{Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:Some(("claude-ref".into(),"claude-account".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None})}else{create_upstream_with_mode(&format!("http://{mock_address}/v1"),"fake-upstream".into(),!native,native).unwrap()};
        let router=Arc::new(Router{token:"fake-gateway".into(),default_id:"test".into(),providers:RwLock::new(BTreeMap::from([("test".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move{loop{let Ok((socket,_))=listener.accept().await else{break};let router=router.clone();let test_file=test_file.clone();tauri::async_runtime::spawn(async move{let service=service_fn(move |request|{let router=router.clone();let test_file=test_file.clone();async move{if antigravity{forward_with_antigravity(request,router,move|_,_,_,prompt,_|{let test_file=test_file.clone();async move{
            if test_tool{
                let has_result=prompt.get("contents").and_then(|v|v.as_array()).is_some_and(|contents|contents.iter().any(|content|content.get("parts").and_then(|v|v.as_array()).is_some_and(|parts|parts.iter().any(|part|part.get("functionResponse").is_some()))));
                if !has_result{
                    if !prompt.pointer("/tools/0/functionDeclarations").and_then(|v|v.as_array()).is_some_and(|tools|tools.iter().any(|v|v.get("name").and_then(|v|v.as_str())==Some("Read"))){return Err("Outil Read indisponible.".into())}
                    let raw=serde_json::json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Read","args":{"file_path":test_file}},"thoughtSignature":"claude-synthetic-signature"}]},"finishReason":"STOP"}]});let reply=crate::google_request::response(&raw,"gemini-3.1-pro-preview")?;return Ok(crate::antigravity_login::GeneratedAnswer{text:String::new(),input_tokens:Some(3),output_tokens:Some(2),truncated:false,reply:Some(reply)})
                }
                if !prompt.to_string().contains("claude-synthetic-signature")||!prompt.to_string().contains("NIVRA_TOOL_OK"){return Err("Retour d’outil ou signature absente.".into())}
            }
            Ok(crate::antigravity_login::GeneratedAnswer{text:"NIVRA_CLAUDE_OK".into(),input_tokens:Some(3),output_tokens:Some(2),truncated:false,reply:None})
        }},false).await}else{forward(request,router).await}}});let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;});}});
        let stdout_path=root.join("stdout.txt");let stderr_path=root.join("stderr.txt");
        let stdout=std::fs::File::create(&stdout_path).unwrap();let stderr=std::fs::File::create(&stderr_path).unwrap();
        let model=if antigravity{"gemini-3.1-pro-preview"}else{"test-model"};
        let mut command=Command::new(executable);command.env_clear();
        for key in ["SystemRoot","WINDIR","PATH","COMSPEC","PATHEXT"]{if let Some(value)=std::env::var_os(key){command.env(key,value);}}
        command.env("CLAUDE_CONFIG_DIR",&config).env("USERPROFILE",&root).env("HOME",&root).env("APPDATA",&root).env("LOCALAPPDATA",&root).env("TEMP",&root).env("TMP",&root)
            .env("ANTHROPIC_BASE_URL",format!("http://{address}")).env("ANTHROPIC_API_KEY","fake-gateway").env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC","1").env("MAX_THINKING_TOKENS","0")
            .env("HTTP_PROXY","http://127.0.0.1:1").env("HTTPS_PROXY","http://127.0.0.1:1").env("NO_PROXY","127.0.0.1,localhost")
            .args(["--bare","--disable-slash-commands","--strict-mcp-config","--no-session-persistence","--tools",if test_tool{"Read"}else{""},"--allowedTools",if test_tool{"Read"}else{""},"--print","--model",model,"Reply with the test marker."])
            .current_dir(&root).stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
        let mut child=command.spawn().expect("Unable to start isolated Claude Code process");let deadline=std::time::Instant::now()+Duration::from_secs(40);
        let status=loop{if let Some(status)=child.try_wait().unwrap(){break status}if std::time::Instant::now()>=deadline{let _=child.kill();let _=child.wait();panic!("Claude Code test timed out")}tokio::time::sleep(Duration::from_millis(100)).await;};
        let mut diagnostic=String::new();std::fs::File::open(&stderr_path).unwrap().take(8192).read_to_string(&mut diagnostic).unwrap();
        let output=std::fs::read_to_string(&stdout_path).unwrap_or_default();
        assert!(status.success(),"Isolated Claude test failed: {diagnostic}; output: {output}");assert!(output.contains("NIVRA_CLAUDE_OK"),"Missing mock completion: {output}");
        if antigravity{return}
        let(path,body)=tokio::time::timeout(Duration::from_secs(3),rx).await.unwrap().unwrap();let body:serde_json::Value=serde_json::from_slice(&body).unwrap();
        assert!(path.starts_with(if native {"/v1/messages"}else{"/v1/chat/completions"}));assert_eq!(body["model"],"test-model");
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Runs a real local Codex client against an entirely simulated provider"]
    async fn codex_cli_smoke_with_isolated_home() {
        use std::{process::{Command,Stdio}, io::Read};
        let codex=std::env::var_os("NIVRA_CODEX_EXECUTABLE").expect("Set NIVRA_CODEX_EXECUTABLE to the local Codex binary");
        let native=std::env::var_os("NIVRA_CODEX_ANTHROPIC").is_some();
        let antigravity=std::env::var_os("NIVRA_CODEX_ANTIGRAVITY").is_some();
        let test_tool=std::env::var_os("NIVRA_CODEX_ANTIGRAVITY_TOOL").is_some();
        let temporary=tempfile::Builder::new().prefix("nivra-codex-smoke-").tempdir_in("target").unwrap();
        let root=temporary.path().canonicalize().unwrap();
        let codex_home=root.join("codex-home"); std::fs::create_dir_all(&codex_home).unwrap();
        let mock=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_address=mock.local_addr().unwrap();
        let (observed_tx, observed_rx)=tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let Ok((socket,_))=mock.accept().await else{return};
            let sender=Arc::new(Mutex::new(Some(observed_tx)));
            let service=service_fn(move |request:Request<Incoming>| {
                let sender=sender.clone();
                async move {
                    let path=request.uri().path().to_string();
                    let body=request.into_body().collect().await.unwrap().to_bytes();
                    if let Some(sender)=sender.lock().unwrap().take(){let _=sender.send((path,body));}
                    let data:&'static [u8]=if native{b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"NIVRA_SMOKE_OK\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"}else{b"data: {\"choices\":[{\"delta\":{\"content\":\"NIVRA_SMOKE_OK\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"};
                    Ok::<_,Infallible>(Response::builder().header("content-type","text/event-stream").body(Full::new(Bytes::from_static(data))).unwrap())
                }
            });
            let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
        });
        let upstream=if antigravity{Arc::new(Upstream{base:upstream_url("https://example.com/v1").unwrap(),key:String::new(),client:reqwest::Client::builder().no_proxy().build().unwrap(),chat_only:true,anthropic_native:false,antigravity:Some(("smoke-ref".into(),"smoke-account".into())),kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None})}else{create_upstream_with_mode(&format!("http://{mock_address}/v1"),"fake-upstream-key".into(),!native,native).unwrap()};
        let router=Arc::new(Router{token:"fake-local-key".into(),default_id:"test".into(),providers:RwLock::new(BTreeMap::from([("test".into(),upstream)]))});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            loop {
                let Ok((socket,_))=listener.accept().await else { break };
                let router=router.clone();
                tauri::async_runtime::spawn(async move {
                    let service=service_fn(move |request|{let router=router.clone();async move{if antigravity{forward_with_antigravity(request,router,|_,_,_,prompt,_|async move{if test_tool{
                        let has_result=prompt.get("contents").and_then(|v|v.as_array()).is_some_and(|contents|contents.iter().any(|content|content.get("parts").and_then(|v|v.as_array()).is_some_and(|parts|parts.iter().any(|part|part.get("functionResponse").is_some()))));
                        if !has_result{
                            if !prompt.pointer("/tools/0/functionDeclarations").and_then(|v|v.as_array()).is_some_and(|tools|tools.iter().any(|v|v.get("name").and_then(|v|v.as_str())==Some("shell_command"))){return Err("Outil shell de test indisponible.".into())}
                            let raw=serde_json::json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"shell_command","args":{"command":"Write-Output NIVRA_TOOL_OK"}},"thoughtSignature":"synthetic-signature"}]},"finishReason":"STOP"}]});
                            let reply=crate::google_request::response(&raw,"test-model")?;return Ok(crate::antigravity_login::GeneratedAnswer{text:String::new(),input_tokens:Some(3),output_tokens:Some(2),truncated:false,reply:Some(reply)})
                        }
                        let has_signature=prompt.to_string().contains("synthetic-signature");let has_tool_output=prompt.to_string().contains("NIVRA_TOOL_OK");if !has_signature||!has_tool_output{return Err("Retour d’outil ou signature absent.".into())}
                    }Ok(crate::antigravity_login::GeneratedAnswer{text:"NIVRA_SMOKE_OK".into(),input_tokens:Some(3),output_tokens:Some(2),truncated:false,reply:None})},false).await}else{forward(request,router).await}}});
                    let _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service).await;
                });
            }
        });
        let stdout_path=root.join("stdout.txt"); let stderr_path=root.join("stderr.txt");
        let stdout=std::fs::File::create(&stdout_path).unwrap(); let stderr=std::fs::File::create(&stderr_path).unwrap();
        let config=["model_provider=\"nivra_test\"".to_string(),"model_providers.nivra_test.name=\"Nivra test\"".into(),format!("model_providers.nivra_test.base_url=\"http://{address}/v1\""),"model_providers.nivra_test.env_key=\"NIVRA_TEST_TOKEN\"".into(),"model_providers.nivra_test.wire_api=\"responses\"".into(),"model_providers.nivra_test.requires_openai_auth=false".into(),"model_providers.nivra_test.supports_websockets=false".into(),"model_providers.nivra_test.request_max_retries=0".into(),"model_providers.nivra_test.stream_max_retries=0".into(),"web_search=\"disabled\"".into()];
        let mut command=Command::new(codex);
        command.env_clear();
        for key in ["SystemRoot","WINDIR","PATH","COMSPEC","PATHEXT"] { if let Some(value)=std::env::var_os(key) { command.env(key,value); } }
        command.env("CODEX_HOME",&codex_home).env("USERPROFILE",&root).env("HOME",&root).env("APPDATA",&root).env("LOCALAPPDATA",&root).env("TEMP",&root).env("TMP",&root)
            .env("NIVRA_TEST_TOKEN","fake-local-key").env("HTTP_PROXY","http://127.0.0.1:1").env("HTTPS_PROXY","http://127.0.0.1:1").env("NO_PROXY","127.0.0.1,localhost")
            .args(["exec","--ignore-user-config","--ignore-rules","--skip-git-repo-check","--ephemeral","--json","--sandbox","read-only","-m","test-model"]);
        for value in &config { command.arg("-c").arg(value); }
        command.arg("Reply with the test marker.").current_dir(&root).stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
        let mut child=command.spawn().expect("Unable to start isolated Codex process");
        let deadline=std::time::Instant::now()+Duration::from_secs(30);
        let status=loop {
            if let Some(status)=child.try_wait().unwrap(){break status}
            if std::time::Instant::now()>=deadline { let _=child.kill();let _=child.wait();panic!("Isolated Codex test timed out") }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let mut diagnostic=String::new();
        std::fs::File::open(&stderr_path).unwrap().take(8192).read_to_string(&mut diagnostic).unwrap();
        let diagnostic=diagnostic.lines().filter(|line| !line.contains("failed to load skill")).collect::<Vec<_>>().join("\n");
        let output=std::fs::read_to_string(&stdout_path).unwrap_or_default();
        assert!(status.success(),"Isolated Codex test failed: {diagnostic}; output: {output}");
        assert!(output.contains("NIVRA_SMOKE_OK"),"Missing mock completion: {output}");
        if antigravity{return}
        let (path,body)=tokio::time::timeout(Duration::from_secs(3),observed_rx).await.unwrap().unwrap();
        let body:serde_json::Value=serde_json::from_slice(&body).unwrap();
        assert_eq!(path,if native{"/v1/messages"}else{"/v1/chat/completions"});
        assert_eq!(body["model"],"test-model");
    }

    #[tokio::test]
    async fn forwards_authenticated_requests_and_rejects_other_clients() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let (socket, _) = upstream_listener.accept().await.unwrap();
            let observed_tx = Arc::new(Mutex::new(Some(observed_tx)));
            let service = service_fn(move |request: Request<Incoming>| {
                let observed_tx = observed_tx.clone();
                async move {
                    let auth = request.headers().get("authorization").unwrap().to_str().unwrap().to_owned();
                    let path = request.uri().path().to_owned();
                    let body = request.into_body().collect().await.unwrap().to_bytes();
                    observed_tx.lock().unwrap().take().unwrap().send((auth, path, body)).unwrap();
                    let body = StreamBody::new(stream::unfold(0, |step| async move {
                        match step {
                            0 => Some((Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"data: hello\n\n"))), 1)),
                            1 => { tokio::time::sleep(Duration::from_millis(400)).await; Some((Ok(Frame::data(Bytes::from_static(b"data: [DONE]\n\n"))), 2)) },
                            _ => None,
                        }
                    }));
                    Ok::<_, Infallible>(Response::builder().header("content-type", "text/event-stream").body(body).unwrap())
                }
            });
            hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).await.unwrap();
        });
        let upstream = Arc::new(Upstream { base: upstream_url(&format!("http://{upstream_addr}/v1")).unwrap(), key:"upstream-secret".into(), client:reqwest::Client::new(), chat_only:false, anthropic_native:false,antigravity:None,kimi:None,xai:None,copilot:None,chatgpt:None,qwen:None,nous:None,kiro:None });
        let router = Arc::new(Router { token:"gateway-secret".into(), default_id:"first".into(), providers:RwLock::new(BTreeMap::from([("first".into(), create_upstream("http://127.0.0.1:1/v1", "wrong-secret".into(), false).unwrap()), ("second".into(), upstream)])) });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            for _ in 0..2 {
                let (socket, _) = listener.accept().await.unwrap();
                let router = router.clone();
                tauri::async_runtime::spawn(async move {
                    let service = service_fn(move |request| forward(request, router.clone()));
                    let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).await;
                });
            }
        });
        let client = reqwest::Client::new();
        let url = format!("http://{address}/v1/chat/completions");
        assert_eq!(client.post(&url).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        let mut result = client.post(&url).bearer_auth("gateway-secret").body("{\"model\":\"second/vendor/model\",\"stream\":true}").send().await.unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(result.headers().get("content-type").unwrap(), "text/event-stream");
        let first = tokio::time::timeout(Duration::from_millis(300), result.chunk()).await.expect("first event must arrive before the second").unwrap().unwrap();
        assert_eq!(first, "data: hello\n\n");
        assert_eq!(result.chunk().await.unwrap().unwrap(), "data: [DONE]\n\n");
        let (auth, path, body) = observed_rx.await.unwrap();
        assert_eq!(auth, "Bearer upstream-secret");
        assert_eq!(path, "/v1/chat/completions");
        let sent: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["model"], "vendor/model");
        assert_eq!(sent["stream"], true);
    }
}
