//! Translation of Chat conversations to the Kiro runtime request format.
use serde_json::{json,Value};
use std::collections::BTreeSet;
use base64::{engine::general_purpose::STANDARD,Engine};

fn name(value:Option<&Value>)->Result<&str,String>{value.and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=64&&s.bytes().all(|c|c.is_ascii_alphanumeric()||matches!(c,b'_'|b'-'))).ok_or_else(||"Nom d’outil Kiro invalide.".into())}
fn identifier(value:Option<&Value>)->Result<&str,String>{value.and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=200&&!s.chars().any(char::is_control)).ok_or_else(||"Identifiant d’outil Kiro invalide.".into())}
pub fn needs_schema(chat:&Value)->bool{["max_tokens","max_completion_tokens","reasoning_effort"].iter().any(|key|chat.get(key).is_some_and(|v|!v.is_null()))}
pub fn apply_options(wire:&mut Value,chat:&Value,schema:Option<&Value>)->Result<(),String>{
 let old=chat.get("max_tokens").filter(|v|!v.is_null());let new=chat.get("max_completion_tokens").filter(|v|!v.is_null());if old.is_some()&&new.is_some(){return Err("Indique une seule limite de sortie Kiro.".into())}
 if !needs_schema(chat){return Ok(())}let owned;let schema=match schema{Some(Value::String(s)) if s.len()<=262144=>{owned=serde_json::from_str::<Value>(s).map_err(|_|"Schéma du modèle Kiro illisible.")?;&owned},Some(v) if v.is_object()=>v,_=>return Err("Ce modèle Kiro ne publie pas les options demandées.".into())};
 let mut options=json!({});if let Some(limit)=old.or(new){let value=limit.as_u64().filter(|v|*v>0).ok_or("Limite de sortie Kiro invalide.")?;let spec=schema.pointer("/properties/max_tokens").filter(|v|v.is_object()).ok_or("Ce modèle Kiro ne prend pas en charge la limite de sortie.")?;if spec.get("type").and_then(Value::as_str)!=Some("integer")||spec.get("minimum").and_then(Value::as_u64).is_some_and(|min|value<min)||spec.get("maximum").and_then(Value::as_u64).is_some_and(|max|value>max){return Err("Limite de sortie hors des valeurs acceptées par ce modèle Kiro.".into())}options["max_tokens"]=json!(value);}
 if let Some(effort)=chat.get("reasoning_effort").filter(|v|!v.is_null()){let effort=effort.as_str().filter(|s|!s.is_empty()&&s.len()<=32).ok_or("Niveau de raisonnement Kiro invalide.")?;let mut matched=false;for field in ["output_config","reasoning"]{let path=format!("/properties/{field}/properties/effort/enum");if schema.pointer(&path).and_then(Value::as_array).is_some_and(|values|values.iter().any(|v|v.as_str()==Some(effort))){options[field]=json!({"effort":effort});matched=true;break}}if !matched{return Err("Niveau de raisonnement indisponible pour ce modèle Kiro.".into())}}
 wire["additionalModelRequestFields"]=options;Ok(())
}
fn user_content(value:Option<&Value>)->Result<(String,Vec<Value>),String>{
 let value=value.ok_or("Message utilisateur Kiro invalide.")?;if let Some(text)=value.as_str(){if text.trim().is_empty(){return Err("Message utilisateur Kiro invalide.".into())}return Ok((text.to_owned(),Vec::new()))}
 let parts=value.as_array().filter(|parts|!parts.is_empty()&&parts.len()<=128).ok_or("Message utilisateur Kiro invalide.")?;let mut text=String::new();let mut images=Vec::new();let mut image_seen=false;
 for part in parts{let part=part.as_object().ok_or("Contenu Kiro invalide.")?;match part.get("type").and_then(Value::as_str){
  Some("text") if part.keys().all(|k|matches!(k.as_str(),"type"|"text"))=>{let content=part.get("text").and_then(Value::as_str).ok_or("Texte Kiro invalide.")?;if image_seen&&!content.is_empty(){return Err("Kiro ne préserve pas cet ordre de texte et d’images.".into())}text.push_str(content)},
  Some("image_url") if part.keys().all(|k|matches!(k.as_str(),"type"|"image_url"))=>{let image=part.get("image_url").and_then(Value::as_object).filter(|o|o.keys().all(|k|matches!(k.as_str(),"url"|"detail"))).ok_or("Image Kiro invalide.")?;if image.get("detail").is_some_and(|v|!v.is_null()&&v.as_str()!=Some("auto")){return Err("Niveau de détail d’image Kiro non pris en charge.".into())}let data=image.get("url").and_then(Value::as_str).ok_or("Image Kiro absente.")?;let (header,data)=data.split_once(',').ok_or("Utilise une image intégrée en base64.")?;let format=match header{"data:image/png;base64"=>"png","data:image/jpeg;base64"=>"jpeg","data:image/gif;base64"=>"gif","data:image/webp;base64"=>"webp",_=>return Err("Format d’image Kiro non pris en charge.".into())};if images.len()>=20||data.is_empty()||data.len()>5*1024*1024||STANDARD.decode(data).is_err(){return Err("Image Kiro invalide ou trop volumineuse.".into())}images.push(json!({"format":format,"source":{"bytes":data}}));image_seen=true},
  _=>return Err("Contenu utilisateur Kiro non pris en charge.".into())
 }}if text.trim().is_empty()&&images.is_empty(){return Err("Message utilisateur Kiro vide.".into())}Ok((text,images))
}

pub fn request(value:&Value,model:&str)->Result<Value,String>{
 let object=value.as_object().ok_or("Requête Kiro invalide.")?;
 if object.iter().any(|(key,v)|!matches!(key.as_str(),"model"|"messages"|"stream"|"tools"|"tool_choice"|"parallel_tool_calls"|"max_tokens"|"max_completion_tokens"|"reasoning_effort")&&!v.is_null()){return Err("Option non prise en charge pour Kiro.".into())}
 if model.is_empty()||model.len()>200||model.chars().any(|c|c.is_control()||c.is_whitespace()){return Err("Modèle Kiro invalide.".into())}
 if object.get("stream").is_some_and(|v|!v.is_boolean()){return Err("Option de flux invalide.".into())}
 if object.get("tool_choice").is_some_and(|v|!v.is_null()&&v.as_str()!=Some("auto")){return Err("Choix d’outil Kiro non pris en charge.".into())}
 if object.get("parallel_tool_calls").is_some_and(|v|!v.is_null()&&!v.is_boolean()){return Err("Option d’appels parallèles Kiro invalide.".into())}
 let mut declarations=Vec::new();let mut declared=BTreeSet::new();
 if let Some(tools)=object.get("tools").filter(|v|!v.is_null()){
  for tool in tools.as_array().filter(|v|v.len()<=128).ok_or("Outils Kiro invalides.")?{
   if tool.get("type").and_then(Value::as_str)!=Some("function")||tool.as_object().is_none_or(|o|o.keys().any(|k|!matches!(k.as_str(),"type"|"function"))){return Err("Outil Kiro non pris en charge.".into())}
   let function=tool.get("function").and_then(Value::as_object).ok_or("Fonction Kiro invalide.")?;if function.keys().any(|k|!matches!(k.as_str(),"name"|"description"|"parameters"|"strict"))||function.get("strict").is_some_and(|v|!v.is_null()&&v.as_bool()!=Some(false)){return Err("Option de fonction Kiro non prise en charge.".into())}
   let id=name(function.get("name"))?;if !declared.insert(id.to_owned()){return Err("Nom d’outil dupliqué.".into())}
   let description=match function.get("description"){None=>format!("Outil {id}"),Some(v)=>v.as_str().filter(|s|!s.trim().is_empty()&&s.len()<=10000).ok_or("Description d’outil invalide.")?.to_owned()};
   let schema=function.get("parameters").cloned().unwrap_or_else(||json!({"type":"object","properties":{}}));if !schema.is_object(){return Err("Schéma d’outil invalide.".into())}
   declarations.push(json!({"toolSpecification":{"name":id,"description":description,"inputSchema":{"json":schema}}}));
  }
 }
 let messages=object.get("messages").and_then(Value::as_array).filter(|v|!v.is_empty()&&v.len()<=512).ok_or("Historique Kiro invalide.")?;
 let mut history=Vec::new();let mut systems=Vec::new();let mut next="user";let mut index=0;let mut text_size=0usize;
 while index<messages.len(){
  let item=messages[index].as_object().ok_or("Message Kiro invalide.")?;let role=item.get("role").and_then(Value::as_str).ok_or("Rôle Kiro manquant.")?;
  if matches!(role,"system"|"developer")&&history.is_empty(){if item.keys().any(|k|!matches!(k.as_str(),"role"|"content")){return Err("Instructions Kiro invalides.".into())}let text=item.get("content").and_then(Value::as_str).filter(|s|!s.trim().is_empty()).ok_or("Instructions Kiro invalides.")?;text_size=text_size.saturating_add(text.len());systems.push(text.to_owned());index+=1;continue}
  if role=="user"{
   if next!="user"||item.keys().any(|k|!matches!(k.as_str(),"role"|"content")){return Err("Ordre des messages Kiro invalide.".into())}
   let(text,images)=user_content(item.get("content"))?;text_size=text_size.saturating_add(text.len());let mut message=json!({"content":text,"modelId":model,"origin":"AI_EDITOR"});if !images.is_empty(){message["images"]=json!(images)}history.push(json!({"userInputMessage":message}));next="assistant";index+=1;continue
  }
  if role=="assistant"{
   if next!="assistant"||item.keys().any(|k|!matches!(k.as_str(),"role"|"content"|"tool_calls")){return Err("Ordre des messages Kiro invalide.".into())}
   let text=match item.get("content"){None|Some(Value::Null)=>"",Some(v)=>v.as_str().ok_or("Réponse Kiro non textuelle.")?};text_size=text_size.saturating_add(text.len());let mut message=json!({"content":text});let mut expected=BTreeSet::new();
   if let Some(raw)=item.get("tool_calls").filter(|v|!v.is_null()){
    let mut uses=Vec::new();for call in raw.as_array().filter(|v|!v.is_empty()&&v.len()<=128).ok_or("Appels d’outils invalides.")?{
     if call.get("type").and_then(Value::as_str)!=Some("function")||call.as_object().is_none_or(|o|o.keys().any(|k|!matches!(k.as_str(),"id"|"type"|"function"))){return Err("Appel d’outil Kiro invalide.".into())}
     let id=identifier(call.get("id"))?;let function=call.get("function").and_then(Value::as_object).filter(|v|v.keys().all(|k|matches!(k.as_str(),"name"|"arguments"))).ok_or("Appel de fonction invalide.")?;let tool=name(function.get("name"))?;
     if !declared.contains(tool)||!expected.insert(id.to_owned()){return Err("Appel d’outil non déclaré ou dupliqué.".into())}
     let arguments=function.get("arguments").and_then(Value::as_str).ok_or("Arguments d’outil manquants.")?;let input:Value=serde_json::from_str(arguments).map_err(|_|"Arguments d’outil illisibles.")?;if !input.is_object(){return Err("Arguments d’outil invalides.".into())}uses.push(json!({"toolUseId":id,"name":tool,"input":input}));
    }message["toolUses"]=json!(uses);
   }
   if text.is_empty()&&expected.is_empty(){return Err("Réponse assistant vide.".into())}history.push(json!({"assistantResponseMessage":message}));index+=1;next="user";
   if !expected.is_empty(){let mut results=Vec::new();while index<messages.len()&&messages[index].get("role").and_then(Value::as_str)==Some("tool"){
     let result=messages[index].as_object().ok_or("Résultat d’outil invalide.")?;if result.keys().any(|k|!matches!(k.as_str(),"role"|"tool_call_id"|"content")){return Err("Résultat d’outil non pris en charge.".into())}let id=identifier(result.get("tool_call_id"))?;if !expected.remove(id){return Err("Résultat d’outil inattendu.".into())}let text=result.get("content").and_then(Value::as_str).ok_or("Résultat d’outil non textuel.")?;text_size=text_size.saturating_add(text.len());results.push(json!({"toolUseId":id,"content":[{"text":text}],"status":"success"}));index+=1;
    }if !expected.is_empty(){return Err("Résultats d’outils manquants.".into())}history.push(json!({"userInputMessage":{"content":"","modelId":model,"origin":"AI_EDITOR","userInputMessageContext":{"toolResults":results}}}));next="assistant";
   }continue
  }
  return Err("Rôle Kiro non pris en charge.".into())
 }
 if text_size>1_000_000{return Err("Historique Kiro trop volumineux.".into())}if next!="assistant"{return Err("La conversation Kiro doit se terminer par une demande ou un résultat d’outil.".into())}
 let mut current=history.pop().and_then(|v|v.get("userInputMessage").cloned()).ok_or("Demande Kiro manquante.")?;
 if !declarations.is_empty(){if current.get("userInputMessageContext").is_none(){current["userInputMessageContext"]=json!({})}current["userInputMessageContext"]["tools"]=json!(declarations);}
 // Kiro does not accept a top-level systemPrompt. Preserve the instructions
 // in the first user turn, including when the current turn is a tool result.
 if !systems.is_empty(){let first=if history.is_empty(){&mut current}else{history[0].get_mut("userInputMessage").ok_or("Historique Kiro invalide.")?};let content=first.get("content").and_then(Value::as_str).ok_or("Message utilisateur Kiro invalide.")?;first["content"]=json!(format!("<system_instructions>\n{}\n</system_instructions>\n\n{}",systems.join("\n\n"),content));}
 Ok(json!({"conversationState":{"chatTriggerType":"MANUAL","conversationId":uuid::Uuid::new_v4().to_string(),"currentMessage":{"userInputMessage":current},"history":history}}))
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn converts_system_text_history_and_tools(){let input=json!({"model":"m","messages":[{"role":"system","content":"Aide"},{"role":"user","content":"Lis"},{"role":"assistant","content":null,"tool_calls":[{"id":"call1","type":"function","function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]},{"role":"tool","tool_call_id":"call1","content":"contenu"}],"tools":[{"type":"function","function":{"name":"read","description":"Lire","parameters":{"type":"object"}}}]});let result=request(&input,"m").unwrap();assert!(result.get("systemPrompt").is_none());assert_eq!(result["conversationState"]["history"][0]["userInputMessage"]["content"],"<system_instructions>\nAide\n</system_instructions>\n\nLis");assert_eq!(result["conversationState"]["history"][1]["assistantResponseMessage"]["toolUses"][0]["input"]["path"],"a");let current=&result["conversationState"]["currentMessage"]["userInputMessage"];assert_eq!(current["userInputMessageContext"]["toolResults"][0]["toolUseId"],"call1");assert_eq!(current["userInputMessageContext"]["tools"][0]["toolSpecification"]["name"],"read");}
 #[test]fn preserves_system_instructions_in_single_turn(){let input=json!({"messages":[{"role":"system","content":"Aide"},{"role":"developer","content":"Réponds en français"},{"role":"user","content":"Bonjour"}]});let result=request(&input,"m").unwrap();assert!(result.get("systemPrompt").is_none());assert_eq!(result["conversationState"]["currentMessage"]["userInputMessage"]["content"],"<system_instructions>\nAide\n\nRéponds en français\n</system_instructions>\n\nBonjour");}
 #[test]fn rejects_orphaned_results_and_unsupported_parameters(){let input=json!({"model":"m","messages":[{"role":"tool","tool_call_id":"missing","content":"x"}]});assert!(request(&input,"m").is_err());let input=json!({"model":"m","temperature":1,"messages":[{"role":"user","content":"x"}]});assert!(request(&input,"m").is_err());}
 #[test]fn accepts_parallel_option_and_non_strict_functions(){let mut input=json!({"model":"m","messages":[{"role":"user","content":"x"}],"parallel_tool_calls":false,"tools":[{"type":"function","function":{"name":"read","strict":false}}]});assert!(request(&input,"m").is_ok());input["tools"][0]["function"]["strict"]=json!(true);assert!(request(&input,"m").is_err());input["tools"][0]["function"]["strict"]=json!(false);input["parallel_tool_calls"]=json!("false");assert!(request(&input,"m").is_err());}
 #[test]fn preserves_inline_images_in_current_and_historical_turns(){let mut input=json!({"messages":[{"role":"user","content":[{"type":"text","text":"Regarde"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]},{"role":"assistant","content":"Oui"},{"role":"user","content":[{"type":"text","text":"Et ceci"},{"type":"image_url","image_url":{"url":"data:image/jpeg;base64,AQ=="}}]}]});let out=request(&input,"m").unwrap();assert_eq!(out["conversationState"]["history"][0]["userInputMessage"]["images"][0]["source"]["bytes"],"AA==");assert_eq!(out["conversationState"]["currentMessage"]["userInputMessage"]["images"][0]["format"],"jpeg");input["messages"][2]["content"][1]["image_url"]["url"]=json!("https://example.com/image.png");assert!(request(&input,"m").is_err());}
 #[test]fn applies_only_options_advertised_by_model(){let schema=json!({"properties":{"max_tokens":{"type":"integer","minimum":1024,"maximum":128000},"output_config":{"properties":{"effort":{"enum":["low","high"]}}}}});let mut chat=json!({"messages":[{"role":"user","content":"Bonjour"}],"max_tokens":2048,"reasoning_effort":"high"});let mut wire=request(&chat,"m").unwrap();apply_options(&mut wire,&chat,Some(&schema)).unwrap();assert_eq!(wire["additionalModelRequestFields"],json!({"max_tokens":2048,"output_config":{"effort":"high"}}));chat["max_tokens"]=json!(100);assert!(apply_options(&mut wire,&chat,Some(&schema)).is_err());chat["max_tokens"]=json!(2048);chat["reasoning_effort"]=json!("max");assert!(apply_options(&mut wire,&chat,Some(&schema)).is_err());assert!(apply_options(&mut wire,&chat,None).is_err());}
}
