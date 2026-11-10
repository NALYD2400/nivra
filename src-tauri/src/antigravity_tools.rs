//! Bounded in-memory recovery of signed Google tool turns for one local gateway session.
use std::{collections::{BTreeMap,BTreeSet},sync::{Mutex,OnceLock},time::{Duration,Instant}};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use crate::google_request::{self,GoogleReply};
struct Entry{reply:GoogleReply,context:[u8;32],expires:Instant,size:usize}
static CACHE:OnceLock<Mutex<BTreeMap<String,Entry>>>=OnceLock::new();
fn cache_key(session:&str,reference:&str,account:&str,model:&str,id:&str)->String{
    let mut hash=Sha256::new();for part in [session,reference,account,model,id]{hash.update((part.len() as u64).to_be_bytes());hash.update(part.as_bytes())}format!("{:x}",hash.finalize())
}
fn clean(cache:&mut BTreeMap<String,Entry>){cache.retain(|_,entry|entry.expires>Instant::now())}
fn calls(reply:&GoogleReply)->Result<&Vec<Value>,String>{reply.chat.pointer("/choices/0/message/tool_calls").and_then(Value::as_array).filter(|c|!c.is_empty()&&c.len()<=128).ok_or("Appel d’outil Google invalide.".into())}
pub fn store(session:&str,reference:&str,account:&str,model:&str,request:&Value,reply:&GoogleReply)->Result<(),String>{
    let declared: BTreeSet<&str>=request.pointer("/tools/0/functionDeclarations").and_then(Value::as_array).ok_or("Google a demandé un outil non déclaré.")?.iter().filter_map(|d|d.get("name").and_then(Value::as_str)).collect();
    let config=request.pointer("/toolConfig/functionCallingConfig");
    if config.and_then(|v|v.get("mode")).and_then(Value::as_str)==Some("NONE"){return Err("Google a appelé un outil malgré son interdiction.".into())}
    let allowed=config.and_then(|v|v.get("allowedFunctionNames")).and_then(Value::as_array);
    let calls=calls(reply)?;for call in calls{let name=call.pointer("/function/name").and_then(Value::as_str).ok_or("Nom d’outil absent.")?;if !declared.contains(name)||allowed.is_some_and(|names|!names.iter().any(|v|v.as_str()==Some(name))){return Err("Google a demandé un outil non autorisé.".into())}}
    let id=calls[0].get("id").and_then(Value::as_str).ok_or("Identifiant d’outil absent.")?;
    let key=cache_key(session,reference,account,model,id);let size=serde_json::to_vec(&reply.model_content).map_err(|_|"Réponse invalide.")?.len()+serde_json::to_vec(&reply.chat).map_err(|_|"Réponse invalide.")?.len();
    if size>512*1024{return Err("Historique d’outil trop volumineux.".into())}
    let mut cache=CACHE.get_or_init(||Mutex::new(BTreeMap::new())).lock().map_err(|_|"Historique d’outil indisponible.")?;clean(&mut cache);
    if cache.contains_key(&key){return Err("Identifiant d’outil déjà utilisé dans cette session.".into())}
    while cache.len()>=64||cache.values().map(|entry|entry.size).sum::<usize>()+size+32>4*1024*1024{let key=cache.iter().min_by_key(|(_,entry)|entry.expires).map(|(key,_)|key.clone()).ok_or("Historique saturé.")?;cache.remove(&key);}
    let context=context_hash(json!({"contents":request.get("contents"),"systemInstruction":request.get("systemInstruction")}))?;
    cache.insert(key,Entry{reply:reply.clone(),context,expires:Instant::now()+Duration::from_secs(1800),size:size+32});Ok(())
}
fn context_hash(value:Value)->Result<[u8;32],String>{let bytes=serde_json::to_vec(&context_for_comparison(value)).map_err(|_|"Historique invalide.")?;Ok(Sha256::digest(bytes).into())}
fn context_for_comparison(mut value:Value)->Value{
    if let Some(parts)=value.pointer_mut("/systemInstruction/parts").and_then(Value::as_array_mut){
        if let Some(text)=parts.first_mut().and_then(|part|part.get_mut("text")){
            if let Some(input)=text.as_str(){
                let (line,rest)=input.split_once('\n').unwrap_or((input,""));
                if line.starts_with("x-anthropic-billing-header: "){
                    if let Some((prefix,tail))=line.rsplit_once("; cch="){
                        if let Some(code)=tail.strip_suffix(';').filter(|code|!code.is_empty()&&code.len()<=64&&code.bytes().all(|c|c.is_ascii_hexdigit())){
                            let _=code;let normalized=format!("{prefix}; cch=<variable>;");
                            *text=Value::String(if rest.is_empty(){normalized}else{format!("{normalized}\n{rest}")});
                        }
                    }
                }
            }
        }
    }
    value
}
fn retrieve(session:&str,reference:&str,account:&str,model:&str,message:&Value,context:&Value)->Result<GoogleReply,String>{
    let input=message.get("tool_calls").and_then(Value::as_array).filter(|v|!v.is_empty()).ok_or("Appel d’outil manquant.")?;
    let id=input[0].get("id").and_then(Value::as_str).ok_or("Identifiant d’outil manquant.")?;
    let key=cache_key(session,reference,account,model,id);let mut cache=CACHE.get_or_init(||Mutex::new(BTreeMap::new())).lock().map_err(|_|"Historique d’outil indisponible.")?;clean(&mut cache);
    let entry=cache.get(&key).ok_or("Historique d’outil introuvable ou expiré ; recommence la conversation.")?;
    if entry.context!=context_hash(context.clone())?{return Err("La conversation ne correspond pas à l’appel d’outil enregistré.".into())}
    let reply=entry.reply.clone();
    let expected=&reply.chat["choices"][0]["message"];
    if expected.get("tool_calls")!=Some(&Value::Array(input.clone()))||expected.get("content").unwrap_or(&Value::Null)!=message.get("content").unwrap_or(&Value::Null){return Err("L’appel d’outil ne correspond pas à la réponse du modèle.".into())}
    Ok(reply)
}
pub fn prepare(session:&str,reference:&str,account:&str,model:&str,value:&Value)->Result<Value,String>{
    let object=value.as_object().ok_or("Requête Chat invalide.")?;
    if object.iter().any(|(key,v)|!matches!(key.as_str(),"model"|"messages"|"stream"|"max_tokens"|"max_completion_tokens"|"tools"|"tool_choice"|"temperature"|"top_p"|"stop")&&!v.is_null()){return Err("Option Chat non prise en charge pour Antigravity.".into())}
    let messages=object.get("messages").and_then(Value::as_array).filter(|v|!v.is_empty()&&v.len()<=128).ok_or("Historique invalide.")?;
    let tools=object.get("tools").cloned().unwrap_or(Value::Null);
    let mut request=google_request::request(&json!({"model":model,"messages":[{"role":"user","content":"placeholder"}],"tools":tools}))?;
    if let Some(choice)=object.get("tool_choice").filter(|v|!v.is_null()){
        let config=match choice.as_str(){
            Some("auto")=>json!({"mode":"AUTO"}),Some("none")=>json!({"mode":"NONE"}),Some("required")=>json!({"mode":"ANY"}),
            Some(_)=>return Err("Choix d’outil non pris en charge.".into()),
            None=>{
                let item=choice.as_object().ok_or("Choix d’outil invalide.")?;
                if item.len()!=2||item.get("type").and_then(Value::as_str)!=Some("function"){return Err("Choix d’outil non pris en charge.".into())}
                let function=item.get("function").and_then(Value::as_object).filter(|f|f.len()==1).ok_or("Choix d’outil invalide.")?;
                let name=function.get("name").and_then(Value::as_str).ok_or("Nom d’outil manquant.")?;
                if !request.pointer("/tools/0/functionDeclarations").and_then(Value::as_array).is_some_and(|tools|tools.iter().any(|tool|tool.get("name").and_then(Value::as_str)==Some(name))){return Err("Outil choisi non déclaré.".into())}
                json!({"mode":"ANY","allowedFunctionNames":[name]})
            }
        };
        if config["mode"]=="ANY"&&request.get("tools").is_none(){return Err("Aucun outil déclaré.".into())}
        request["toolConfig"]=json!({"functionCallingConfig":config});
    }

    let mut contents=Vec::new();let mut system=Vec::new();let mut next="user";let mut started=false;let mut index=0;let mut bytes=0usize;
    while index<messages.len(){
        let item=messages[index].as_object().ok_or("Message invalide.")?;let role=item.get("role").and_then(Value::as_str).ok_or("Rôle manquant.")?;
        if role=="assistant"&&item.get("tool_calls").is_some_and(|v|!v.is_null()){
            if next!="assistant"||item.keys().any(|k|!matches!(k.as_str(),"role"|"content"|"tool_calls")){return Err("Historique d’outil invalide.".into())}
            let context=json!({"contents":contents,"systemInstruction":if system.is_empty(){Value::Null}else{json!({"parts":system})}});
            let reply=retrieve(session,reference,account,model,&messages[index],&context)?;index+=1;let mut results=Vec::new();
            while index<messages.len()&&messages[index].get("role").and_then(Value::as_str)==Some("tool"){
                let result=messages[index].as_object().ok_or("Résultat d’outil invalide.")?;if result.keys().any(|k|!matches!(k.as_str(),"role"|"tool_call_id"|"content")){return Err("Résultat d’outil non pris en charge.".into())}
                let id=result.get("tool_call_id").and_then(Value::as_str).ok_or("Identifiant de résultat manquant.")?;let output=result.get("content").and_then(Value::as_str).ok_or("Résultat d’outil non textuel.")?;bytes=bytes.saturating_add(output.len());results.push((id.to_owned(),output.to_owned()));index+=1;
            }
            contents.extend(google_request::tool_results(&reply,&results)?);next="assistant";continue
        }
        if item.keys().any(|k|!matches!(k.as_str(),"role"|"content")){return Err("Métadonnées Chat non prises en charge.".into())}
        let content=item.get("content").ok_or("Contenu manquant.")?;
        if matches!(role,"system"|"developer")&&!started{let text=content.as_str().filter(|s|!s.trim().is_empty()).ok_or("Instructions texte requises.")?;bytes=bytes.saturating_add(text.len());system.push(json!({"text":text}));index+=1;continue}
        let parts=google_request::content_parts(content,role)?;for part in &parts{if let Some(text)=part.get("text").and_then(Value::as_str){bytes=bytes.saturating_add(text.len())}}
        if role=="user"&&next=="assistant"&&contents.last().is_some_and(|last: &Value|last.get("role").and_then(Value::as_str)==Some("user")&&last.get("parts").and_then(Value::as_array).is_some_and(|parts|parts.iter().all(|part|part.get("text").is_some()||part.get("inlineData").is_some()))){
            contents.last_mut().unwrap()["parts"].as_array_mut().unwrap().extend(parts);index+=1;continue
        }
        if role!=next{return Err(format!("Ordre des messages invalide : rôle {role}, attendu {next}."))}started=true;contents.push(json!({"role":if role=="assistant"{"model"}else{"user"},"parts":parts}));next=if role=="user"{"assistant"}else{"user"};index+=1;
    }
    if bytes>128000||contents.is_empty()||next!="assistant"{return Err("Historique incomplet ou trop volumineux.".into())}
    request["contents"]=json!(contents);if !system.is_empty(){request["systemInstruction"]=json!({"parts":system})}google_request::generation_options(value,&mut request)?;Ok(request)
}

#[cfg(test)] mod tests{
 use super::*;
 #[test]fn preserves_image_context_across_signed_tool_roundtrip(){
  let mut input=json!({"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"Lis cette image"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}],"tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}}]});
  let request=prepare("image-session","r","a","m",&input).unwrap();assert_eq!(request["contents"][0]["parts"][1]["inlineData"]["data"],"AA==");
  let reply=google_request::response(&json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read","args":{}},"thoughtSignature":"signature"}]},"finishReason":"STOP"}]}),"m").unwrap();store("image-session","r","a","m",&request,&reply).unwrap();
  let assistant=reply.chat["choices"][0]["message"].clone();let id=assistant["tool_calls"][0]["id"].as_str().unwrap().to_owned();input["messages"].as_array_mut().unwrap().push(assistant);input["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":id,"content":"ok"}));
  let resumed=prepare("image-session","r","a","m",&input).unwrap();assert_eq!(resumed["contents"][0]["parts"][1]["inlineData"]["data"],"AA==");assert_eq!(resumed["contents"][1]["parts"][0]["thoughtSignature"],"signature");
  input["messages"][0]["content"][1]["image_url"]["url"]=json!("data:image/png;base64,AQ==");assert!(prepare("image-session","r","a","m",&input).is_err());
 }
 #[test] fn maps_tool_choices_and_rejects_undeclared_functions(){
  let mut input=json!({"model":"m","messages":[{"role":"user","content":"bonjour"}],"tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}}]});
  for (choice,mode) in [(json!("auto"),"AUTO"),(json!("none"),"NONE"),(json!("required"),"ANY")]{input["tool_choice"]=choice;assert_eq!(prepare("session","ref","account","m",&input).unwrap()["toolConfig"]["functionCallingConfig"]["mode"],mode)}
  input["tool_choice"]=json!({"type":"function","function":{"name":"read"}});let request=prepare("session","ref","account","m",&input).unwrap();assert_eq!(request["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][0],"read");
  input["tool_choice"]=json!({"type":"function","function":{"name":"other"}});assert!(prepare("session","ref","account","m",&input).is_err());
  input["tool_choice"]=json!("required");input["tools"]=json!([]);assert!(prepare("session","ref","account","m",&input).is_err());
 }
 #[test] fn only_normalizes_variable_claude_billing_checksum(){
  let input=|line:&str|json!({"systemInstruction":{"parts":[{"text":format!("{line}\nInstruction utilisateur")}]},"contents":[{"role":"user","parts":[{"text":"Salut"}]}]});
  let first=input("x-anthropic-billing-header: cc_version=1; cc_entrypoint=cli; cch=abcdef;");let second=input("x-anthropic-billing-header: cc_version=1; cc_entrypoint=cli; cch=123456;");assert_eq!(context_for_comparison(first.clone()),context_for_comparison(second));
  let changed=input("x-anthropic-billing-header: cc_version=2; cc_entrypoint=cli; cch=123456;");assert_ne!(context_for_comparison(first.clone()),context_for_comparison(changed));
  let mut changed=input("x-anthropic-billing-header: cc_version=1; cc_entrypoint=cli; cch=123456;");changed["contents"][0]["parts"][0]["text"]=json!("Autre");assert_ne!(context_for_comparison(first),context_for_comparison(changed));
 }
 #[test] fn merges_consecutive_user_messages_as_separate_parts(){
  let input=json!({"model":"m","messages":[{"role":"user","content":"Contexte"},{"role":"user","content":"Question"}],"tools":[{"type":"function","function":{"name":"read"}}]});
  let request=prepare("session","ref","account","m",&input).unwrap();assert_eq!(request["contents"].as_array().unwrap().len(),1);assert_eq!(request["contents"][0]["parts"],json!([{"text":"Contexte"},{"text":"Question"}]));
 }
 #[test] fn refuses_tool_calls_when_none_or_different_function_selected(){
  let reply=google_request::response(&json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read","args":{}}}]},"finishReason":"STOP"}]}),"m").unwrap();
  let mut request=json!({"contents":[],"tools":[{"functionDeclarations":[{"name":"read"},{"name":"other"}]}],"toolConfig":{"functionCallingConfig":{"mode":"NONE"}}});
  assert!(store("choice-session","r","a","m",&request,&reply).is_err());request["toolConfig"]["functionCallingConfig"]=json!({"mode":"ANY","allowedFunctionNames":["other"]});assert!(store("choice-session","r","a","m",&request,&reply).is_err());
 }
}
