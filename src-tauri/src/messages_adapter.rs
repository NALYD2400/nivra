//! Translate the supported Anthropic Messages subset to Chat Completions.
use std::collections::{BTreeMap,BTreeSet};
use bytes::Bytes;
use serde_json::{json, Value};

pub(crate) fn text_or_parts(value:&Value)->Result<Value,String> {
    if let Some(text)=value.as_str(){return Ok(json!(text))}
    let parts=value.as_array().ok_or("Contenu de message invalide.")?;
    let mut converted=Vec::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => converted.push(json!({"type":"text","text":part.get("text").and_then(Value::as_str).ok_or("Texte invalide.")?})),
            Some("image") => {
                let source=part.get("source").ok_or("Source d'image manquante.")?;
                let url=match source.get("type").and_then(Value::as_str) {
                    Some("url") => source.get("url").and_then(Value::as_str).ok_or("URL d'image manquante.")?.to_string(),
                    Some("base64") => {
                        let media=source.get("media_type").and_then(Value::as_str).filter(|value|matches!(*value,"image/jpeg"|"image/png"|"image/gif"|"image/webp")).ok_or("Type d'image non pris en charge.")?;
                        let data=source.get("data").and_then(Value::as_str).ok_or("Données d'image manquantes.")?;
                        format!("data:{media};base64,{data}")
                    },
                    _=>return Err("Source d'image non prise en charge.".into()),
                };
                converted.push(json!({"type":"image_url","image_url":{"url":url}}));
            }
            _=>return Err("Bloc de contenu Messages non pris en charge.".into()),
        }
    }
    Ok(Value::Array(converted))
}

pub fn request(input:&Value)->Result<(Value,BTreeSet<String>),String> {
    let source=input.as_object().ok_or("Requête Messages invalide.")?;
    for key in ["container","context_management","thread"] {
        if source.get(key).is_some_and(|value|!value.is_null() && value!=false) {return Err(format!("L'option {key} n'est pas prise en charge par cette traduction."))}
    }
    if input.get("thinking").is_some_and(|value|!value.is_null() && value.get("type").and_then(Value::as_str)!=Some("disabled")) {return Err("Le raisonnement étendu n'est pas encore pris en charge par cette traduction.".into())}
    let model=input.get("model").and_then(Value::as_str).filter(|model|!model.is_empty()).ok_or("Modèle manquant.")?;
    let max=input.get("max_tokens").and_then(Value::as_u64).filter(|value|*value>0).ok_or("max_tokens invalide.")?;
    let mut messages=Vec::new();
    if let Some(system)=input.get("system").filter(|value|!value.is_null()) {
        let text=if let Some(text)=system.as_str(){text.to_string()}else{
            let parts=system.as_array().ok_or("Instructions système invalides.")?;
            let mut result=Vec::new();for part in parts {
                if part.get("type").and_then(Value::as_str)!=Some("text"){return Err("Instructions système non textuelles non prises en charge.".into())}
                result.push(part.get("text").and_then(Value::as_str).ok_or("Instructions système invalides.")?);
            } result.join("\n")
        };
        messages.push(json!({"role":"system","content":text}));
    }
    let input_messages=input.get("messages").and_then(Value::as_array).ok_or("Liste de messages manquante.")?;
    for message in input_messages {
        let role=message.get("role").and_then(Value::as_str).ok_or("Rôle de message manquant.")?;
        if !matches!(role,"user"|"assistant"|"system"|"developer"){return Err(format!("Rôle de message non pris en charge : {role}."))}
        let content=message.get("content").ok_or("Contenu de message manquant.")?;
        if content.is_string(){messages.push(json!({"role":role,"content":content}));continue}
        if matches!(role,"system"|"developer") {
            if content.as_array().is_some_and(|parts|parts.iter().any(|part|part.get("type").and_then(Value::as_str)!=Some("text"))){return Err("Instructions non textuelles non prises en charge.".into())}
            messages.push(json!({"role":role,"content":text_or_parts(content)?}));continue
        }
        let parts=content.as_array().ok_or("Contenu de message invalide.")?;
        let mut ordinary=Vec::new();let mut calls=Vec::new();
        for part in parts {
            match part.get("type").and_then(Value::as_str) {
                Some("text"|"image")=>{if role=="assistant"&&!calls.is_empty(){return Err("Texte après un appel d'outil non pris en charge.".into())}ordinary.push(part.clone())},
                Some("tool_use") if role=="assistant"=>{
                    let id=part.get("id").and_then(Value::as_str).ok_or("Appel d'outil sans identifiant.")?;
                    let name=part.get("name").and_then(Value::as_str).ok_or("Appel d'outil sans nom.")?;
                    let arguments=part.get("input").filter(|value|value.is_object()).ok_or("Entrée d'outil invalide.")?.to_string();
                    calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}}));
                }
                Some("tool_result") if role=="user"=>{
                    if !ordinary.is_empty() {messages.push(json!({"role":"user","content":text_or_parts(&Value::Array(std::mem::take(&mut ordinary)))?}));}
                    let id=part.get("tool_use_id").and_then(Value::as_str).ok_or("Résultat d'outil sans identifiant.")?;
                    let value=part.get("content").cloned().unwrap_or(json!(""));
                    let content=if value.is_string(){value}else{
                        let blocks=value.as_array().ok_or("Résultat d'outil invalide.")?;
                        let mut texts=Vec::new();for block in blocks {
                            if block.get("type").and_then(Value::as_str)!=Some("text"){return Err("Résultat d'outil non textuel non pris en charge.".into())}
                            texts.push(block.get("text").and_then(Value::as_str).ok_or("Résultat d'outil invalide.")?);
                        } json!(texts.join("\n"))
                    };
                    let content=if part.get("is_error").and_then(Value::as_bool)==Some(true){json!(format!("Tool error: {}",content.as_str().unwrap_or("")))}else{content};
                    messages.push(json!({"role":"tool","tool_call_id":id,"content":content}));
                }
                _=>return Err("Ce bloc Messages n'est pas encore pris en charge.".into()),
            }
        }
        if role=="assistant" {
            let content=if ordinary.is_empty(){Value::Null}else{text_or_parts(&Value::Array(ordinary))?};
            let mut converted=json!({"role":"assistant","content":content});
            if !calls.is_empty(){converted["tool_calls"]=json!(calls)}
            messages.push(converted);
        } else if !ordinary.is_empty(){messages.push(json!({"role":"user","content":text_or_parts(&Value::Array(ordinary))?}));}
    }
    let mut result=json!({"model":model,"messages":messages,"max_tokens":max,"stream":input.get("stream").and_then(Value::as_bool).unwrap_or(false)});
    for key in ["temperature","top_p"] {if let Some(value)=source.get(key){result[key]=value.clone()}}
    if let Some(value)=source.get("stop_sequences"){result["stop"]=value.clone()}
    let mut allowed=BTreeSet::new();
    if let Some(tools)=input.get("tools").filter(|value|!value.is_null()){
        let mut converted=Vec::new();
        for tool in tools.as_array().ok_or("Liste d'outils invalide.")? {
            if tool.get("type").and_then(Value::as_str).is_some_and(|kind|kind!="custom") {return Err("Outil intégré ou spécialisé non pris en charge.".into())}
            let name=tool.get("name").and_then(Value::as_str).ok_or("Outil sans nom.")?;
            if name.is_empty()||!allowed.insert(name.to_string()){return Err("Nom d'outil invalide ou en double.".into())}
            let schema=tool.get("input_schema").filter(|schema|schema.is_object()).ok_or("Schéma d'outil invalide.")?;
            let mut function=json!({"name":name,"parameters":schema});
            if let Some(description)=tool.get("description"){function["description"]=description.clone()}
            converted.push(json!({"type":"function","function":function}));
        }
        result["tools"]=json!(converted);
    }
    if let Some(choice)=input.get("tool_choice").filter(|value|!value.is_null()) {
        result["tool_choice"]=match choice.get("type").and_then(Value::as_str) {
            Some("auto")=>json!("auto"),Some("any")=>json!("required"),Some("none")=>json!("none"),
            Some("tool")=>{let name=choice.get("name").and_then(Value::as_str).ok_or("Choix d'outil invalide.")?;if !allowed.contains(name){return Err("Choix d'outil non déclaré.".into())}json!({"type":"function","function":{"name":name}})},
            _=>return Err("Choix d'outil non pris en charge.".into()),
        };
        if choice.get("disable_parallel_tool_use").and_then(Value::as_bool)==Some(true){result["parallel_tool_calls"]=json!(false)}
    }
    Ok((result,allowed))
}

fn stop_reason(reason:&str)->Result<&'static str,String>{match reason{"stop"=>Ok("end_turn"),"length"=>Ok("max_tokens"),"tool_calls"=>Ok("tool_use"),"content_filter"=>Err("Réponse filtrée par le fournisseur.".into()),_=>Err("État final du fournisseur non pris en charge.".into())}}
fn tokens(usage:&Value,key:&str)->u64{usage.get(key).and_then(Value::as_u64).unwrap_or(0)}
fn new_id()->String{format!("msg_{}",uuid::Uuid::new_v4().simple())}

pub fn non_stream(value:&Value,model:&str,allowed:&BTreeSet<String>)->Result<Value,String>{
    let choice=value.get("choices").and_then(Value::as_array).and_then(|choices|choices.first()).ok_or("Réponse sans choix.")?;
    let message=choice.get("message").ok_or("Réponse sans message.")?;
    if ["reasoning_content","reasoning","reasoning_details"].iter().any(|key|message.get(key).is_some_and(|value|!value.is_null()&&value!=""&&value!=&json!([]))){return Err("Sortie de raisonnement non prise en charge.".into())}
    if message.get("refusal").and_then(Value::as_str).is_some_and(|value|!value.is_empty())||message.get("function_call").is_some_and(|value|!value.is_null()){return Err("Refus ou ancien appel de fonction non pris en charge.".into())}
    let mut content=Vec::new();
    if let Some(text)=message.get("content").and_then(Value::as_str).filter(|text|!text.is_empty()){content.push(json!({"type":"text","text":text}))}
    if message.get("content").is_some_and(|value|!value.is_null()&&!value.is_string()){return Err("Contenu du fournisseur non pris en charge.".into())}
    if let Some(calls)=message.get("tool_calls").filter(|value|!value.is_null()){
        for call in calls.as_array().ok_or("Appels d'outils invalides.")? {
            if call.get("type").and_then(Value::as_str)!=Some("function"){return Err("Appel d'outil non pris en charge.".into())}
            let id=call.get("id").and_then(Value::as_str).ok_or("Appel d'outil sans identifiant.")?;
            let name=call.pointer("/function/name").and_then(Value::as_str).ok_or("Appel d'outil sans nom.")?;
            if !allowed.contains(name){return Err("Le fournisseur a appelé un outil non déclaré.".into())}
            let args=call.pointer("/function/arguments").and_then(Value::as_str).ok_or("Arguments d'outil manquants.")?;
            let input:Value=serde_json::from_str(args).map_err(|_|"Arguments d'outil JSON invalides.")?;
            if !input.is_object(){return Err("Entrée d'outil non prise en charge.".into())}
            content.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
        }
    }
    let reason=stop_reason(choice.get("finish_reason").and_then(Value::as_str).ok_or("État final manquant.")?)?;
    let has_tools=content.iter().any(|item|item["type"]=="tool_use");
    if (reason=="tool_use")!=has_tools{return Err("État final incohérent pour les outils.".into())}
    let usage=&value["usage"];
    Ok(json!({"id":new_id(),"type":"message","role":"assistant","model":model,"content":content,"stop_reason":reason,"stop_sequence":null,"usage":{"input_tokens":tokens(usage,"prompt_tokens"),"output_tokens":tokens(usage,"completion_tokens")}}))
}

#[derive(Default)]
struct Tool{ id:String,name:String,args:String }
pub struct StreamAdapter{model:String,id:String,pending:Vec<u8>,data:Vec<String>,text_started:bool,tools:BTreeMap<u64,Tool>,finish:Option<String>,usage:Value,done:bool,allowed:BTreeSet<String>}
impl StreamAdapter {
    pub fn new(model:String,allowed:BTreeSet<String>)->Self{Self{model,id:new_id(),pending:Vec::new(),data:Vec::new(),text_started:false,tools:BTreeMap::new(),finish:None,usage:Value::Null,done:false,allowed}}
    fn event(kind:&str,mut value:Value)->Bytes{value["type"]=json!(kind);Bytes::from(format!("event: {kind}\ndata: {value}\n\n"))}
    pub fn start(&mut self)->Vec<Bytes>{self.start_with_input_tokens(0)}
    pub fn start_with_input_tokens(&mut self,input:u64)->Vec<Bytes>{vec![Self::event("message_start",json!({"message":{"id":self.id,"type":"message","role":"assistant","content":[],"model":self.model,"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":input,"output_tokens":0}}}))]}
    pub fn is_done(&self)->bool{self.done}
    pub fn ingest(&mut self,chunk:&[u8])->Result<Vec<Bytes>,String>{
        if self.done{return Ok(vec![])}
        self.pending.extend_from_slice(chunk);if self.pending.len()>2*1024*1024{return Err("Événement trop volumineux.".into())}
        let mut events=Vec::new();while let Some(end)=self.pending.iter().position(|byte|*byte==b'\n'){
            let mut line:Vec<u8>=self.pending.drain(..=end).collect();line.pop();if line.last()==Some(&b'\r'){line.pop();}
            let line=String::from_utf8(line).map_err(|_|"Flux invalide.")?;
            if line.is_empty(){if !self.data.is_empty(){let data=std::mem::take(&mut self.data).join("\n");events.extend(self.process(&data)?)}if self.done{break}}
            else if let Some(data)=line.strip_prefix("data:"){let data=data.strip_prefix(' ').unwrap_or(data);if self.data.len()>=8192||data.len()+self.data.iter().map(String::len).sum::<usize>()>2*1024*1024{return Err("Événement trop volumineux.".into())}self.data.push(data.to_string())}
        }Ok(events)
    }
    fn process(&mut self,data:&str)->Result<Vec<Bytes>,String>{
        if data=="[DONE]"{return self.complete()}
        let chunk:Value=serde_json::from_str(data).map_err(|_|"Événement JSON invalide.")?;
        if chunk.get("error").is_some(){return Err("Le fournisseur a interrompu la réponse.".into())}
        if let Some(value)=chunk.get("usage").filter(|value|!value.is_null()){self.usage=value.clone()}
        let Some(choice)=chunk.get("choices").and_then(Value::as_array).and_then(|items|items.first()) else{return Ok(vec![])};
        if let Some(reason)=choice.get("finish_reason").and_then(Value::as_str){self.finish=Some(reason.to_string())}
        let delta=&choice["delta"];
        if ["reasoning_content","reasoning","reasoning_details"].iter().any(|key|delta.get(key).is_some_and(|value|!value.is_null()&&value!=""&&value!=&json!([])))||delta.get("refusal").and_then(Value::as_str).is_some_and(|value|!value.is_empty()) {return Err("Raisonnement ou refus en streaming non pris en charge.".into())}
        if delta.get("function_call").is_some_and(|value|!value.is_null()){return Err("Ancien format d'appel de fonction non pris en charge.".into())}
        if delta.get("content").is_some_and(|value|!value.is_null()&&!value.is_string()){return Err("Contenu de flux non pris en charge.".into())}
        let mut events=Vec::new();
        if let Some(text)=delta.get("content").and_then(Value::as_str).filter(|value|!value.is_empty()){
            if !self.text_started{self.text_started=true;events.push(Self::event("content_block_start",json!({"index":0,"content_block":{"type":"text","text":""}})))}
            events.push(Self::event("content_block_delta",json!({"index":0,"delta":{"type":"text_delta","text":text}})));
        }
        if let Some(calls)=delta.get("tool_calls").and_then(Value::as_array){for call in calls{
            if call.get("type").and_then(Value::as_str).is_some_and(|kind|kind!="function"){return Err("Appel d'outil non pris en charge.".into())}
            let index=call.get("index").and_then(Value::as_u64).filter(|value|*value<128).ok_or("Index d'outil invalide.")?;
            let tool=self.tools.entry(index).or_default();
            if let Some(id)=call.get("id").and_then(Value::as_str){tool.id.push_str(id)}
            if let Some(name)=call.pointer("/function/name").and_then(Value::as_str){tool.name.push_str(name)}
            if let Some(args)=call.pointer("/function/arguments").and_then(Value::as_str){tool.args.push_str(args)}
            if tool.id.len()>4096||tool.name.len()>4096||tool.args.len()>8*1024*1024{return Err("Appel d'outil trop volumineux.".into())}
        }}
        if self.tools.values().map(|tool|tool.args.len()).sum::<usize>()>16*1024*1024{return Err("Appels d'outils trop volumineux.".into())}
        Ok(events)
    }
    fn complete(&mut self)->Result<Vec<Bytes>,String>{
        let reason=stop_reason(self.finish.as_deref().ok_or("Flux terminé sans état final.")?)?;
        if (reason=="tool_use")!=!self.tools.is_empty(){return Err("État final incohérent pour les outils.".into())}
        let mut events=Vec::new();let mut index=0;
        if self.text_started{events.push(Self::event("content_block_stop",json!({"index":index})));index+=1}
        for (_,tool) in std::mem::take(&mut self.tools){
            if tool.id.is_empty()||tool.name.is_empty(){return Err("Appel d'outil incomplet.".into())}
            if !self.allowed.contains(&tool.name){return Err("Le fournisseur a appelé un outil non déclaré.".into())}
            let args:Value=serde_json::from_str(&tool.args).map_err(|_|"Arguments d'outil JSON invalides.")?;
            if !args.is_object(){return Err("Entrée d'outil non prise en charge.".into())}
            events.push(Self::event("content_block_start",json!({"index":index,"content_block":{"type":"tool_use","id":tool.id,"name":tool.name,"input":{}}})));
            events.push(Self::event("content_block_delta",json!({"index":index,"delta":{"type":"input_json_delta","partial_json":tool.args}})));
            events.push(Self::event("content_block_stop",json!({"index":index})));index+=1;
        }
        events.push(Self::event("message_delta",json!({"delta":{"stop_reason":reason,"stop_sequence":null},"usage":{"output_tokens":tokens(&self.usage,"completion_tokens")}})));
        events.push(Self::event("message_stop",json!({})));self.done=true;Ok(events)
    }
    pub fn fail(&mut self,message:&str)->Vec<Bytes>{if self.done{return vec![]}self.done=true;vec![Self::event("error",json!({"error":{"type":"api_error","message":message}}))]}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_messages_images_tools_and_results() {
        let input=json!({"model":"example","max_tokens":300,"system":[{"type":"text","text":"Be helpful"}],"messages":[
            {"role":"user","content":[{"type":"text","text":"Look"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"read","input":{"path":"a"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"missing","is_error":true}]}],
            "tools":[{"name":"read","input_schema":{"type":"object"}}],"tool_choice":{"type":"auto","disable_parallel_tool_use":true}});
        let (chat,allowed)=request(&input).unwrap();
        assert_eq!(chat["messages"][0]["content"],"Be helpful");
        assert_eq!(chat["messages"][1]["content"][1]["image_url"]["url"],"data:image/png;base64,aGVsbG8=");
        assert_eq!(chat["messages"][2]["tool_calls"][0]["id"],"call_1");
        assert_eq!(chat["messages"][3]["content"],"Tool error: missing");
        assert_eq!(chat["parallel_tool_calls"],false);
        assert!(allowed.contains("read"));
    }
    #[test]
    fn rejects_unsupported_features_and_unknown_tools(){
        assert!(request(&json!({"model":"m","max_tokens":10,"messages":[],"thinking":{"type":"enabled"}})).is_err());
        assert!(request(&json!({"model":"m","max_tokens":10,"messages":[],"tools":[{"type":"web_search_20250305","name":"web"}]})).is_err());
        let answer=json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"c","type":"function","function":{"name":"unknown","arguments":"{}"}}]}}]});
        assert!(non_stream(&answer,"m",&BTreeSet::new()).is_err());
    }
    #[test]
    fn translates_stream_with_fragmented_tool_arguments(){
        let chunks=[json!({"choices":[{"delta":{"content":"Salut"},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{\"path\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"x\"}"}}]},"finish_reason":"tool_calls"}]})];
        let source=format!("{}data: [DONE]\r\n\r\n",chunks.iter().map(|chunk|format!("data: {chunk}\r\n\r\n")).collect::<String>());
        let mut adapter=StreamAdapter::new("provider/model".into(),BTreeSet::from(["read".into()]));
        let mut events=adapter.start();for bytes in source.as_bytes().chunks(5){events.extend(adapter.ingest(bytes).unwrap())}
        let events:Vec<Value>=events.iter().map(|event|serde_json::from_str(std::str::from_utf8(event).unwrap().lines().find_map(|line|line.strip_prefix("data: ")).unwrap()).unwrap()).collect();
        assert_eq!(events.first().unwrap()["type"],"message_start");
        assert!(events.iter().any(|event|event["type"]=="content_block_delta"&&event["delta"]["text"]=="Salut"));
        assert!(events.iter().any(|event|event["type"]=="content_block_start"&&event["content_block"]["name"]=="read"));
        assert!(events.iter().any(|event|event["type"]=="content_block_delta"&&event["delta"]["partial_json"]=="{\"path\":\"x\"}"));
        assert_eq!(events.last().unwrap()["type"],"message_stop");
        assert!(adapter.is_done());
    }
}
