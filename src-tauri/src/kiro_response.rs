//! Assemble Kiro events into an OpenAI-compatible Chat completion.
use std::collections::{BTreeMap,BTreeSet};
use serde_json::{json,Value};
use crate::kiro_eventstream::{Decoder,Event};

#[derive(Default)]struct Tool{name:String,input:String,stopped:bool}
pub struct Response{decoder:Decoder,model:String,allowed:BTreeSet<String>,text:String,tools:BTreeMap<String,Tool>,order:Vec<String>,finish:Option<String>,usage:Option<Value>,metered:bool,reasoning:String,signature:String,redacted:String}
impl Response{
 pub fn new(model:String,allowed:BTreeSet<String>)->Self{Self{decoder:Decoder::default(),model,allowed,text:String::new(),tools:BTreeMap::new(),order:Vec::new(),finish:None,usage:None,metered:false,reasoning:String::new(),signature:String::new(),redacted:String::new()}}
 pub fn ingest(&mut self,bytes:&[u8])->Result<(),String>{for event in self.decoder.ingest(bytes)?{self.event(event)?}Ok(())}
 fn event(&mut self,event:Event)->Result<(),String>{
  let message=event.headers.get(":message-type").map(String::as_str).ok_or("Type de message Kiro absent.")?;
  if matches!(message,"exception"|"error"){let kind=event.headers.get(":exception-type").or_else(||event.headers.get(":error-code")).map(String::as_str);return Err(match kind{Some("ThrottlingException"|"TooManyRequestsException"|"ServiceQuotaExceededException")=>"__kiro_rate_limit",Some("AccessDeniedException"|"ForbiddenException")=>"__kiro_forbidden",Some("ValidationException"|"InvalidRequestException")=>"__kiro_invalid",Some("ServiceUnavailableException")=>"__kiro_unavailable",_=>"Kiro a interrompu la réponse."}.into())}if message!="event"{return Err("Type de message Kiro inconnu.".into())}
  let kind=event.headers.get(":event-type").map(String::as_str).ok_or("Événement Kiro absent.")?;let value:Value=if kind=="initial-response"&&event.payload.is_empty(){json!({})}else{serde_json::from_slice(&event.payload).map_err(|_|"Événement Kiro illisible.")?};
  match kind{
   "assistantResponseEvent"=>{if self.finish.is_some(){return Err("Texte Kiro après la fin de réponse.".into())}let text=value.get("content").and_then(Value::as_str).ok_or("Texte Kiro invalide.")?;if self.text.len().saturating_add(text.len())>8*1024*1024{return Err("Réponse Kiro trop volumineuse.".into())}self.text.push_str(text)},
   "toolUseEvent"=>{
    if self.finish.is_some(){return Err("Outil Kiro après la fin de réponse.".into())}
    let id=value.get("toolUseId").and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=200&&!s.chars().any(char::is_control)).ok_or("Identifiant d’outil Kiro invalide.")?;
    if !self.tools.contains_key(id){if self.tools.len()>=128{return Err("Trop d’outils Kiro.".into())}self.order.push(id.to_owned());self.tools.insert(id.to_owned(),Tool::default());}
    let tool=self.tools.get_mut(id).unwrap();if tool.stopped{return Err("Outil Kiro déjà terminé.".into())}
    if let Some(name)=value.get("name").and_then(Value::as_str).filter(|s|!s.is_empty()){if !self.allowed.contains(name)||(!tool.name.is_empty()&&tool.name!=name){return Err("Outil Kiro non déclaré ou incohérent.".into())}tool.name=name.to_owned();}
    if let Some(input)=value.get("input").filter(|v|!v.is_null()){let part=input.as_str().ok_or("Arguments Kiro invalides.")?;if tool.input.len().saturating_add(part.len())>1024*1024{return Err("Arguments Kiro trop volumineux.".into())}tool.input.push_str(part)}
    if let Some(stop)=value.get("stop"){if stop.as_bool().ok_or("État d’outil Kiro invalide.")?{tool.stopped=true}}
   },
   "metadataEvent"=>{
    if let Some(reason)=value.get("stopReason").and_then(Value::as_str).filter(|s|!s.is_empty()){if self.finish.is_some(){return Err("Fin Kiro répétée.".into())}self.finish=Some(match reason{"END_TURN"=>"stop","TOOL_USE"=>"tool_calls","MAX_TOKENS"|"MODEL_CONTEXT_WINDOW_EXCEEDED"=>"length","CONTENT_FILTERED"=>"content_filter",_=>return Err("État final Kiro non pris en charge.".into())}.into());}
    if let Some(usage)=value.get("tokenUsage").and_then(Value::as_object){let input=usage.get("uncachedInputTokens").and_then(Value::as_u64);let output=usage.get("outputTokens").and_then(Value::as_u64);if let (Some(input),Some(output))=(input,output){let prompt=input.saturating_add(usage.get("cacheReadInputTokens").and_then(Value::as_u64).unwrap_or(0)).saturating_add(usage.get("cacheWriteInputTokens").and_then(Value::as_u64).unwrap_or(0));self.usage=Some(json!({"prompt_tokens":prompt,"completion_tokens":output,"total_tokens":usage.get("totalTokens").and_then(Value::as_u64).unwrap_or(prompt.saturating_add(output))}));}}
   },
   "meteringEvent"=>{if value.get("unit").and_then(Value::as_str)==Some("credit")&&value.get("usage").and_then(Value::as_f64).is_some_and(|v|v.is_finite()&&v>=0.0){self.metered=true}},
   "reasoningContentEvent"=>{for(key,target)in [("text",&mut self.reasoning),("signature",&mut self.signature),("redactedContent",&mut self.redacted)]{if let Some(part)=value.get(key).filter(|v|!v.is_null()){let part=part.as_str().ok_or("Raisonnement Kiro invalide.")?;if target.len().saturating_add(part.len())>1024*1024{return Err("Raisonnement Kiro trop volumineux.".into())}target.push_str(part)}}},
   "initial-response"|"messageMetadataEvent"|"contextUsageEvent"|"supplementaryWebLinksEvent"=>{if !value.is_object(){return Err("Métadonnées Kiro invalides.".into())}},
   _=>return Err("Événement Kiro non pris en charge.".into())
  }Ok(())
 }
 pub fn finish(self)->Result<Value,String>{
  self.decoder.finish()?;let inferred=self.finish.is_none();let finish=match self.finish{Some(finish)=>finish,None if self.metered&&(!self.text.is_empty()||!self.order.is_empty())=>if self.order.is_empty(){"stop".into()}else{"tool_calls".into()},None=>return Err("Fin de réponse Kiro manquante.".into())};let mut calls=Vec::new();
  for id in &self.order{let tool=self.tools.get(id).unwrap();if !tool.stopped||tool.name.is_empty(){return Err("Appel d’outil Kiro incomplet.".into())}let parsed:Value=serde_json::from_str(&tool.input).map_err(|_|"Arguments d’outil Kiro illisibles.")?;if !parsed.is_object(){return Err("Arguments d’outil Kiro invalides.".into())}calls.push(json!({"id":id,"type":"function","function":{"name":tool.name,"arguments":tool.input}}));}
  if (finish=="tool_calls")!=!calls.is_empty(){return Err("Fin d’outil Kiro incohérente.".into())}
  let mut message=json!({"role":"assistant","content":if self.text.is_empty(){Value::Null}else{json!(self.text)}});if !calls.is_empty(){message["tool_calls"]=json!(calls)}
  let mut result=json!({"id":format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()),"object":"chat.completion","created":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),"model":self.model,"choices":[{"index":0,"message":message,"finish_reason":finish}]});if let Some(usage)=self.usage{result["usage"]=usage}if inferred{result["nivra_finish_reason_inferred"]=json!(true)}
  if !self.reasoning.is_empty()||!self.signature.is_empty()||!self.redacted.is_empty(){let reasoning=if !self.redacted.is_empty()&&self.reasoning.is_empty()&&self.signature.is_empty(){json!({"redactedContent":self.redacted})}else if self.redacted.is_empty()&&!self.reasoning.is_empty()&&!self.signature.is_empty(){json!({"reasoningText":{"text":self.reasoning,"signature":self.signature}})}else{return Err("Raisonnement signé Kiro incomplet ou incohérent.".into())};result["_nivra_kiro_reasoning"]=reasoning;}
  Ok(result)
 }
}
/// Rough text-only estimate for clients whose protocol requires token counts.
/// Images are refused because their token cost cannot be inferred from base64 size.
pub fn estimate_text_usage(chat:&Value,result:&Value)->Result<Value,String>{
 let serialized=chat.to_string();if serialized.contains("data:image/"){return Err("Kiro n'a pas communiqué les tokens pour cette image.".into())}
 let message=result.pointer("/choices/0/message").ok_or("Réponse Kiro invalide.")?.to_string();let input=((serialized.len() as u64).saturating_add(3)/4).max(1);let output=((message.len() as u64).saturating_add(3)/4).max(1);Ok(json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":input.saturating_add(output)}))
}
/// Encode a completed response as SSE. The upstream response is collected first,
/// so transport errors cannot be mistaken for a successful partial response.
pub fn buffered_stream(result:&Value)->Result<Vec<u8>,String>{
 let id=result.get("id").and_then(Value::as_str).ok_or("Réponse Kiro invalide.")?;let model=result.get("model").and_then(Value::as_str).ok_or("Réponse Kiro invalide.")?;let created=result.get("created").and_then(Value::as_u64).ok_or("Réponse Kiro invalide.")?;
 let choice=result.pointer("/choices/0").ok_or("Réponse Kiro invalide.")?;let message=choice.get("message").ok_or("Réponse Kiro invalide.")?;let finish=choice.get("finish_reason").and_then(Value::as_str).ok_or("Réponse Kiro incomplète.")?;
 let make=|delta:Value,reason:Value|json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":reason}]});
 let mut events=vec![make(json!({"role":"assistant"}),Value::Null)];
 if let Some(text)=message.get("content").and_then(Value::as_str){events.push(make(json!({"content":text}),Value::Null));}
 if let Some(calls)=message.get("tool_calls").and_then(Value::as_array){let calls:Vec<_>=calls.iter().enumerate().map(|(index,call)|json!({"index":index,"id":call["id"],"type":"function","function":call["function"]})).collect();events.push(make(json!({"tool_calls":calls}),Value::Null));}
 let mut last=make(json!({}),json!(finish));if let Some(usage)=result.get("usage"){last["usage"]=usage.clone()}events.push(last);
 Ok((events.iter().map(|v|format!("data: {v}\n\n")).collect::<String>()+"data: [DONE]\n\n").into_bytes())
}
#[cfg(test)]mod tests{use super::*;
 fn event(kind:&str,value:Value)->Event{Event{headers:BTreeMap::from([(":message-type".into(),"event".into()),(":event-type".into(),kind.into())]),payload:value.to_string().into_bytes()}}
 #[test]fn reconstructs_fragmented_binary_stream(){
  let events=[("initial-response",json!({"conversationId":""})),("assistantResponseEvent",json!({"content":"Bonjour"})),("toolUseEvent",json!({"toolUseId":"c1","name":"read","input":"{\"path\":"})),("toolUseEvent",json!({"toolUseId":"c1","input":"\"a\"}","stop":true})),("metadataEvent",json!({"stopReason":"TOOL_USE"})),("messageMetadataEvent",json!({"conversationId":"id","utteranceId":"turn"}))];
  let mut wire=Vec::new();for(kind,payload)in events{wire.extend(crate::kiro_eventstream::tests::frame(kind,payload.to_string().as_bytes()));}
  let mut response=Response::new("m".into(),BTreeSet::from(["read".into()]));for chunk in wire.chunks(7){response.ingest(chunk).unwrap()}
  let result=response.finish().unwrap();assert_eq!(result["choices"][0]["message"]["content"],"Bonjour");assert_eq!(result["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],"{\"path\":\"a\"}");assert_eq!(result["choices"][0]["finish_reason"],"tool_calls");
 }
 #[test]fn reconstructs_text_tool_fragments_and_usage(){let mut response=Response::new("m".into(),BTreeSet::from(["read".into()]));response.event(event("assistantResponseEvent",json!({"content":"Je lis."}))).unwrap();response.event(event("toolUseEvent",json!({"toolUseId":"c1","name":"read","input":"{\"path\":"}))).unwrap();response.event(event("toolUseEvent",json!({"toolUseId":"c1","input":"\"a\"}","stop":true}))).unwrap();response.event(event("metadataEvent",json!({"stopReason":"TOOL_USE","tokenUsage":{"uncachedInputTokens":3,"cacheReadInputTokens":2,"outputTokens":4}}))).unwrap();let result=response.finish().unwrap();assert_eq!(result["choices"][0]["message"]["content"],"Je lis.");assert_eq!(result["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],"{\"path\":\"a\"}");assert_eq!(result["choices"][0]["finish_reason"],"tool_calls");assert_eq!(result["usage"]["prompt_tokens"],5);}
 #[test]fn rejects_unauthorized_or_incomplete_events(){let mut bad=Response::new("m".into(),BTreeSet::new());assert!(bad.event(event("toolUseEvent",json!({"toolUseId":"c","name":"read","input":"{}","stop":true}))).is_err());let mut incomplete=Response::new("m".into(),BTreeSet::from(["read".into()]));incomplete.event(event("toolUseEvent",json!({"toolUseId":"c","name":"read","input":"{"}))).unwrap();incomplete.event(event("metadataEvent",json!({"stopReason":"TOOL_USE"}))).unwrap();assert!(incomplete.finish().is_err());assert!(Response::new("m".into(),BTreeSet::new()).finish().is_err());}
 #[test]fn completed_tool_response_encodes_chat_stream(){let mut response=Response::new("m".into(),BTreeSet::from(["read".into()]));response.event(event("toolUseEvent",json!({"toolUseId":"c","name":"read","input":"{}","stop":true}))).unwrap();response.event(event("metadataEvent",json!({"stopReason":"TOOL_USE"}))).unwrap();let s=String::from_utf8(buffered_stream(&response.finish().unwrap()).unwrap()).unwrap();assert!(s.contains("\"tool_calls\""));assert!(s.contains("\"finish_reason\":\"tool_calls\""));assert!(s.ends_with("data: [DONE]\n\n"));}
 #[test]fn accepts_clean_metered_response_without_stop_metadata(){let mut response=Response::new("m".into(),BTreeSet::new());response.event(event("assistantResponseEvent",json!({"content":"Bonjour"}))).unwrap();response.event(event("meteringEvent",json!({"unit":"credit","usage":0.01}))).unwrap();let out=response.finish().unwrap();assert_eq!(out["choices"][0]["finish_reason"],"stop");assert_eq!(out["nivra_finish_reason_inferred"],true);assert!(out.get("usage").is_none());let mut missing=Response::new("m".into(),BTreeSet::new());missing.event(event("assistantResponseEvent",json!({"content":"incomplet"}))).unwrap();missing.event(event("meteringEvent",json!({"unit":"tokens","usage":10}))).unwrap();assert!(missing.finish().is_err());}
 #[test]fn estimates_only_text_when_usage_is_missing(){let chat=json!({"messages":[{"role":"user","content":"Bonjour"}]});let result=json!({"choices":[{"message":{"role":"assistant","content":"Salut"}}]});let usage=estimate_text_usage(&chat,&result).unwrap();assert!(usage["prompt_tokens"].as_u64().unwrap()>0);assert_eq!(usage["total_tokens"],usage["prompt_tokens"].as_u64().unwrap()+usage["completion_tokens"].as_u64().unwrap());assert!(estimate_text_usage(&json!({"image":"data:image/png;base64,AA=="}),&result).is_err());}
 #[test]fn detects_throttling_inside_stream(){let mut response=Response::new("m".into(),BTreeSet::new());let error=Event{headers:BTreeMap::from([(":message-type".into(),"exception".into()),(":exception-type".into(),"ThrottlingException".into())]),payload:b"{}".to_vec()};assert_eq!(response.event(error).unwrap_err(),"__kiro_rate_limit");}
}
