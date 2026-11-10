//! Assemble an OpenAI-compatible chat event stream for non-streaming clients.
use serde_json::{json,Value};
use std::collections::BTreeMap;

#[derive(Default)]
struct Tool { id:String, name:String, arguments:String }

pub fn complete(bytes:&[u8],model:&str)->Result<Value,String>{
 let text=std::str::from_utf8(bytes).map_err(|_|"Flux Grok illisible.")?.replace("\r\n","\n");
 let mut content=String::new();let mut reasoning=String::new();let mut tools:BTreeMap<u64,Tool>=BTreeMap::new();let mut finish=None;let mut done=false;let mut usage=None;let mut id=None;let mut created=None;
 for frame in text.split("\n\n"){
  let data=frame.lines().filter_map(|line|line.strip_prefix("data:").map(|s|s.strip_prefix(' ').unwrap_or(s))).collect::<Vec<_>>().join("\n");
  if data.is_empty(){continue}if data=="[DONE]"{done=true;break}
  let item:Value=serde_json::from_str(&data).map_err(|_|"Événement Grok illisible.")?;
  if item.get("error").is_some_and(|v|!v.is_null()){return Err("Grok a interrompu la réponse.".into())}
  if id.is_none(){id=item.get("id").and_then(Value::as_str).map(str::to_owned)}
  if created.is_none(){created=item.get("created").and_then(Value::as_u64)}
  if let Some(v)=item.get("usage").filter(|v|v.is_object()){usage=Some(v.clone())}
  let choices=item.get("choices").and_then(Value::as_array).ok_or("Choix Grok invalides.")?;
  for choice in choices{
   if choice.get("index").and_then(Value::as_u64)!=Some(0){return Err("Plusieurs choix Grok ne sont pas encore pris en charge.".into())}
   if let Some(reason)=choice.get("finish_reason").and_then(Value::as_str){if finish.is_some(){return Err("Fin de réponse Grok répétée.".into())}finish=Some(reason.to_owned())}
   let delta=choice.get("delta").and_then(Value::as_object).ok_or("Fragment Grok invalide.")?;
   if delta.get("refusal").and_then(Value::as_str).is_some_and(|s|!s.is_empty()){return Err("La réponse Grok a été refusée.".into())}
   if let Some(s)=delta.get("content").and_then(Value::as_str){content.push_str(s)}
   if let Some(s)=delta.get("reasoning_content").and_then(Value::as_str){reasoning.push_str(s)}
   if let Some(calls)=delta.get("tool_calls").filter(|v|!v.is_null()){
    for call in calls.as_array().filter(|v|v.len()<=128).ok_or("Outils Grok invalides.")?{
     let index=call.get("index").and_then(Value::as_u64).filter(|v|*v<128).ok_or("Index d’outil Grok invalide.")?;
     if call.get("type").and_then(Value::as_str).is_some_and(|s|s!="function"){return Err("Type d’outil Grok inconnu.".into())}
     let tool=tools.entry(index).or_default();
     if let Some(s)=call.get("id").and_then(Value::as_str){tool.id.push_str(s)}
     if let Some(s)=call.pointer("/function/name").and_then(Value::as_str){tool.name.push_str(s)}
     if let Some(s)=call.pointer("/function/arguments").and_then(Value::as_str){tool.arguments.push_str(s)}
    }
   }
  }
 }
 if !done||finish.is_none(){return Err("Flux Grok terminé prématurément.".into())}
 let mut message=json!({"role":"assistant","content":if content.is_empty(){Value::Null}else{json!(content)}});
 if !reasoning.is_empty(){message["reasoning_content"]=json!(reasoning)}
 if !tools.is_empty(){let mut calls=Vec::new();for(index,tool)in tools{if index!=calls.len() as u64||tool.id.is_empty()||tool.name.is_empty(){return Err("Appel d’outil Grok incomplet.".into())}calls.push(json!({"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":tool.arguments}}));}message["tool_calls"]=json!(calls);}
 let mut result=json!({"id":id.unwrap_or_else(||format!("chatcmpl-{}",uuid::Uuid::new_v4().simple())),"object":"chat.completion","created":created.unwrap_or(0),"model":model,"choices":[{"index":0,"message":message,"finish_reason":finish}]});
 if let Some(usage)=usage{result["usage"]=usage}Ok(result)
}

#[cfg(test)]mod tests{
 use super::*;
 #[test]fn reconstructs_content_tools_and_usage(){let stream=concat!("data: {\"id\":\"x\",\"created\":12,\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Bon\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"p\\\":\"}}]},\"finish_reason\":null}]}\n\n","data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"jour\",\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n","data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n","data: [DONE]\n\n");let value=complete(stream.as_bytes(),"grok/m").unwrap();assert_eq!(value["choices"][0]["message"]["content"],"Bonjour");assert_eq!(value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],"{\"p\":1}");assert_eq!(value["usage"]["total_tokens"],5);assert!(complete(stream.replace("data: [DONE]\n\n","").as_bytes(),"m").is_err());}
}
