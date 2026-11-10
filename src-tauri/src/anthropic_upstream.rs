//! Translate Chat Completions to Anthropic Messages and back.
//! This module only handles the subset that can be represented faithfully.
use std::collections::BTreeMap;
use bytes::Bytes;
use serde_json::{json,Value};

fn chat_parts(content:&Value)->Result<Vec<Value>,String>{
    if let Some(text)=content.as_str(){return Ok(vec![json!({"type":"text","text":text})])}
    let parts=content.as_array().ok_or("Contenu Chat invalide.")?;
    let mut result=Vec::new();
    for part in parts{
        match part.get("type").and_then(Value::as_str){
            Some("text")=>result.push(json!({"type":"text","text":part.get("text").and_then(Value::as_str).ok_or("Texte invalide.")?})),
            Some("image_url")=>{
                let image=part.pointer("/image_url/url").and_then(Value::as_str).ok_or("URL d'image manquante.")?;
                let source=if image.starts_with("data:"){
                    let (meta,data)=image.strip_prefix("data:").and_then(|value|value.split_once(",")).ok_or("Image encodée invalide.")?;
                    let media=meta.strip_suffix(";base64").filter(|media|matches!(*media,"image/png"|"image/jpeg"|"image/gif"|"image/webp")).ok_or("Format d'image non pris en charge.")?;
                    json!({"type":"base64","media_type":media,"data":data})
                }else{json!({"type":"url","url":image})};
                result.push(json!({"type":"image","source":source}));
            }
            _=>return Err("Contenu Chat non pris en charge pour Anthropic.".into()),
        }
    }
    Ok(result)
}
fn push_message(messages:&mut Vec<Value>,role:&str,parts:Vec<Value>){
    if parts.is_empty(){return}
    if let Some(last)=messages.last_mut().filter(|last|last["role"]==role){last["content"].as_array_mut().unwrap().extend(parts)}
    else{messages.push(json!({"role":role,"content":parts}))}
}

pub fn request(chat:&Value)->Result<Value,String>{
    let model=chat.get("model").and_then(Value::as_str).filter(|value|!value.is_empty()).ok_or("Modèle manquant.")?;
    let mut system=Vec::new();let mut messages=Vec::new();
    let history=chat.get("messages").and_then(Value::as_array).ok_or("Messages manquants.")?;
    for message in history{
        let role=message.get("role").and_then(Value::as_str).ok_or("Rôle de message manquant.")?;
        match role{
            "system"|"developer"=>{
                if !messages.is_empty(){return Err("Instructions système en milieu de conversation non prises en charge.".into())}
                let parts=chat_parts(message.get("content").ok_or("Instructions manquantes.")?)?;
                if parts.iter().any(|part|part["type"]!="text"){return Err("Instructions non textuelles non prises en charge.".into())}system.extend(parts);
            }
            "user"=>push_message(&mut messages,"user",chat_parts(message.get("content").ok_or("Contenu manquant.")?)?),
            "assistant"=>{
                let mut parts=match message.get("content"){Some(value) if !value.is_null()=>chat_parts(value)?,_=>Vec::new()};
                if let Some(calls)=message.get("tool_calls").filter(|value|!value.is_null()){
                    for call in calls.as_array().ok_or("Appels d'outils invalides.")?{
                        if call.get("type").and_then(Value::as_str)!=Some("function"){return Err("Type d'outil non pris en charge.".into())}
                        let id=call.get("id").and_then(Value::as_str).ok_or("Appel d'outil sans identifiant.")?;
                        let name=call.pointer("/function/name").and_then(Value::as_str).ok_or("Appel d'outil sans nom.")?;
                        let arguments=call.pointer("/function/arguments").and_then(Value::as_str).ok_or("Arguments d'outil manquants.")?;
                        let input:Value=serde_json::from_str(arguments).map_err(|_|"Arguments d'outil JSON invalides.")?;
                        if !input.is_object(){return Err("Arguments d'outil non pris en charge.".into())}
                        parts.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
                    }
                }push_message(&mut messages,"assistant",parts);
            }
            "tool"=>{
                let id=message.get("tool_call_id").and_then(Value::as_str).ok_or("Résultat sans identifiant d'outil.")?;
                let output=message.get("content").and_then(Value::as_str).ok_or("Résultat d'outil non textuel non pris en charge.")?;
                push_message(&mut messages,"user",vec![json!({"type":"tool_result","tool_use_id":id,"content":output})]);
            }
            _=>return Err("Rôle Chat non pris en charge.".into()),
        }
    }
    if messages.is_empty(){return Err("Conversation vide.".into())}
    let max=chat.get("max_tokens").and_then(Value::as_u64).unwrap_or(8192);
    if max==0{return Err("Limite de sortie invalide.".into())}
    let mut result=json!({"model":model,"messages":messages,"max_tokens":max,"stream":chat.get("stream").and_then(Value::as_bool).unwrap_or(false)});
    if !system.is_empty(){result["system"]=json!(system)}
    for key in ["temperature","top_p"] {if let Some(value)=chat.get(key){result[key]=value.clone()}}
    if let Some(value)=chat.get("stop") {result["stop_sequences"]=if value.is_string(){json!([value])}else{value.clone()}}
    if let Some(effort)=chat.get("reasoning_effort").and_then(Value::as_str){
        if !matches!(effort,"low"|"medium"|"high"|"max"|"xhigh"){return Err("Niveau de raisonnement non pris en charge par Anthropic.".into())}
        result["output_config"]=json!({"effort":effort});
    }
    if chat.get("response_format").is_some_and(|value|!value.is_null()){return Err("Format structuré non pris en charge par cette traduction Anthropic.".into())}
    if let Some(tools)=chat.get("tools").filter(|value|!value.is_null()){
        let mut converted=Vec::new();for tool in tools.as_array().ok_or("Outils invalides.")?{
            if tool.get("type").and_then(Value::as_str)!=Some("function"){return Err("Type d'outil non pris en charge.".into())}
            let function=tool.get("function").ok_or("Outil invalide.")?;
            let name=function.get("name").and_then(Value::as_str).ok_or("Outil sans nom.")?;
            let schema=function.get("parameters").filter(|value|value.is_object()).cloned().unwrap_or(json!({"type":"object","properties":{}}));
            let mut tool=json!({"name":name,"input_schema":schema});
            for key in ["description","strict"]{if let Some(value)=function.get(key){tool[key]=value.clone()}}
            converted.push(tool);
        }result["tools"]=json!(converted);
    }
    let disable_parallel=chat.get("parallel_tool_calls").and_then(Value::as_bool)==Some(false);
    if let Some(choice)=chat.get("tool_choice").filter(|value|!value.is_null()){
        let mut value=match choice.as_str(){
            Some("auto")=>json!({"type":"auto"}),Some("required")=>json!({"type":"any"}),Some("none")=>json!({"type":"none"}),
            Some(_)=>return Err("Choix d'outil non pris en charge.".into()),
            None=>{let name=choice.pointer("/function/name").and_then(Value::as_str).ok_or("Choix d'outil invalide.")?;json!({"type":"tool","name":name})},
        };
        if disable_parallel{value["disable_parallel_tool_use"]=json!(true)}result["tool_choice"]=value;
    }else if disable_parallel{result["tool_choice"]=json!({"type":"auto","disable_parallel_tool_use":true})}
    Ok(result)
}

fn reason(value:&str)->Result<&'static str,String>{match value{"end_turn"|"stop_sequence"=>Ok("stop"),"max_tokens"=>Ok("length"),"tool_use"=>Ok("tool_calls"),_=>Err("État final Anthropic non pris en charge.".into())}}
fn token(usage:&Value,key:&str)->u64{usage.get(key).and_then(Value::as_u64).unwrap_or(0)}
pub fn non_stream(message:&Value)->Result<Value,String>{
    let blocks=message.get("content").and_then(Value::as_array).ok_or("Réponse Anthropic sans contenu.")?;
    let mut text=String::new();let mut calls=Vec::new();
    for block in blocks{match block.get("type").and_then(Value::as_str){
        Some("text")=>{if !calls.is_empty(){return Err("Texte après un appel d'outil non pris en charge.".into())}text.push_str(block.get("text").and_then(Value::as_str).ok_or("Texte Anthropic invalide.")?)},
        Some("tool_use")=>{
            let id=block.get("id").and_then(Value::as_str).ok_or("Appel d'outil sans identifiant.")?;
            let name=block.get("name").and_then(Value::as_str).ok_or("Appel d'outil sans nom.")?;
            let input=block.get("input").filter(|value|value.is_object()).ok_or("Entrée d'outil invalide.")?;
            calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":input.to_string()}}));
        }
        _=>return Err("Bloc Anthropic non pris en charge.".into()),
    }}
    let finish=reason(message.get("stop_reason").and_then(Value::as_str).ok_or("État final manquant.")?)?;
    if (finish=="tool_calls")!=!calls.is_empty(){return Err("État final incohérent pour les outils.".into())}
    let mut assistant=json!({"role":"assistant","content":if text.is_empty(){Value::Null}else{json!(text)}});
    if !calls.is_empty(){assistant["tool_calls"]=json!(calls)}
    let usage=&message["usage"];let input=token(usage,"input_tokens");let output=token(usage,"output_tokens");
    Ok(json!({"choices":[{"index":0,"message":assistant,"finish_reason":finish}],"usage":{"prompt_tokens":input,"completion_tokens":output,"total_tokens":input+output}}))
}

#[derive(Default)]
struct Block{kind:String,tool_index:usize,open:bool,has_initial_input:bool,has_delta:bool,arguments:String}
pub struct StreamBridge{pending:Vec<u8>,data:Vec<String>,started:bool,done:bool,blocks:BTreeMap<u64,Block>,next_tool:usize,finish:Option<String>,input_tokens:u64,output_tokens:u64}
impl StreamBridge{
    pub fn new()->Self{Self{pending:Vec::new(),data:Vec::new(),started:false,done:false,blocks:BTreeMap::new(),next_tool:0,finish:None,input_tokens:0,output_tokens:0}}
    pub fn is_done(&self)->bool{self.done}
    fn chat(delta:Value,finish:Value)->Bytes{Bytes::from(format!("data: {}\n\n",json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})))}
    pub fn ingest(&mut self,chunk:&[u8])->Result<Vec<Bytes>,String>{
        if self.done{return Ok(vec![])}self.pending.extend_from_slice(chunk);if self.pending.len()>2*1024*1024{return Err("Événement Anthropic trop volumineux.".into())}
        let mut output=Vec::new();while let Some(end)=self.pending.iter().position(|byte|*byte==b'\n'){
            let mut line:Vec<u8>=self.pending.drain(..=end).collect();line.pop();if line.last()==Some(&b'\r'){line.pop();}
            let line=String::from_utf8(line).map_err(|_|"Flux Anthropic invalide.")?;
            if line.is_empty(){if !self.data.is_empty(){let data=std::mem::take(&mut self.data).join("\n");output.extend(self.process(&data)?)}if self.done{break}}
            else if let Some(data)=line.strip_prefix("data:"){let data=data.strip_prefix(' ').unwrap_or(data);if self.data.len()>=8192||data.len()+self.data.iter().map(String::len).sum::<usize>()>2*1024*1024{return Err("Événement Anthropic trop volumineux.".into())}self.data.push(data.into())}
        }Ok(output)
    }
    fn process(&mut self,data:&str)->Result<Vec<Bytes>,String>{
        let event:Value=serde_json::from_str(data).map_err(|_|"Événement Anthropic invalide.")?;
        let kind=event.get("type").and_then(Value::as_str).ok_or("Type d'événement manquant.")?;
        let mut out=Vec::new();
        match kind{
            "ping"=>{},
            "message_start"=>{if self.started{return Err("Début de message répété.".into())}self.started=true;self.input_tokens=token(&event["message"]["usage"],"input_tokens");},
            "content_block_start"=>{
                if !self.started{return Err("Bloc reçu avant le message.".into())}
                let index=event.get("index").and_then(Value::as_u64).filter(|index|*index<256).ok_or("Index de bloc invalide.")?;
                if self.blocks.contains_key(&index){return Err("Bloc répété.".into())}
                let content=&event["content_block"];let kind=content.get("type").and_then(Value::as_str).ok_or("Bloc sans type.")?;
                let mut block=Block{kind:kind.into(),open:true,..Default::default()};
                match kind{
                    "text"=>{if self.next_tool>0{return Err("Texte après un appel d'outil non pris en charge.".into())}if let Some(text)=content.get("text").and_then(Value::as_str).filter(|text|!text.is_empty()){out.push(Self::chat(json!({"content":text}),Value::Null))}},
                    "tool_use"=>{
                        let id=content.get("id").and_then(Value::as_str).filter(|value|!value.is_empty()).ok_or("Outil sans identifiant.")?;
                        let name=content.get("name").and_then(Value::as_str).filter(|value|!value.is_empty()).ok_or("Outil sans nom.")?;
                        block.tool_index=self.next_tool;self.next_tool+=1;
                        let input=content.get("input").filter(|value|value.is_object()).ok_or("Entrée d'outil invalide.")?;
                        let args=if input.as_object().unwrap().is_empty(){String::new()}else{block.has_initial_input=true;input.to_string()};block.arguments=args.clone();
                        out.push(Self::chat(json!({"tool_calls":[{"index":block.tool_index,"id":id,"type":"function","function":{"name":name,"arguments":args}}]}),Value::Null));
                    }
                    _=>return Err("Bloc Anthropic non pris en charge.".into()),
                }self.blocks.insert(index,block);
            }
            "content_block_delta"=>{
                let index=event.get("index").and_then(Value::as_u64).ok_or("Index de bloc manquant.")?;
                let block=self.blocks.get_mut(&index).filter(|block|block.open).ok_or("Bloc inconnu ou fermé.")?;let delta=&event["delta"];
                match (block.kind.as_str(),delta.get("type").and_then(Value::as_str)){
                    ("text",Some("text_delta")) if self.next_tool==0=>out.push(Self::chat(json!({"content":delta.get("text").and_then(Value::as_str).ok_or("Texte manquant.")?}),Value::Null)),
                    ("tool_use",Some("input_json_delta")) if !block.has_initial_input=>{let args=delta.get("partial_json").and_then(Value::as_str).ok_or("Arguments manquants.")?;if block.arguments.len()+args.len()>8*1024*1024{return Err("Arguments d'outil trop volumineux.".into())}block.has_delta=true;block.arguments.push_str(args);out.push(Self::chat(json!({"tool_calls":[{"index":block.tool_index,"function":{"arguments":args}}]}),Value::Null))},
                    _=>return Err("Fragment Anthropic non pris en charge.".into()),
                }
            }
            "content_block_stop"=>{let index=event.get("index").and_then(Value::as_u64).ok_or("Index de bloc manquant.")?;let block=self.blocks.get_mut(&index).filter(|block|block.open).ok_or("Bloc inconnu ou fermé.")?;if block.kind=="tool_use"{if !block.has_initial_input&&!block.has_delta{block.arguments="{}".into();out.push(Self::chat(json!({"tool_calls":[{"index":block.tool_index,"function":{"arguments":"{}"}}]}),Value::Null));}let parsed:Value=serde_json::from_str(&block.arguments).map_err(|_|"Arguments d'outil JSON invalides.")?;if !parsed.is_object(){return Err("Arguments d'outil non pris en charge.".into())}}block.open=false;}
            "message_delta"=>{if let Some(value)=event.pointer("/delta/stop_reason").and_then(Value::as_str){self.finish=Some(reason(value)?.into())}self.output_tokens=token(&event["usage"],"output_tokens");}
            "message_stop"=>{
                if !self.started||self.blocks.values().any(|block|block.open){return Err("Message Anthropic incomplet.".into())}
                let finish=self.finish.as_deref().ok_or("État final manquant.")?;
                if (finish=="tool_calls")!=(self.next_tool>0){return Err("État final incohérent pour les outils.".into())}
                out.push(Self::chat(json!({}),json!(finish)));
                out.push(Bytes::from(format!("data: {}\n\n",json!({"choices":[],"usage":{"prompt_tokens":self.input_tokens,"completion_tokens":self.output_tokens,"total_tokens":self.input_tokens+self.output_tokens}}))));
                out.push(Bytes::from_static(b"data: [DONE]\n\n"));self.done=true;
            }
            "error"=>return Err("Le fournisseur Anthropic a interrompu la réponse.".into()),
            _=>return Err("Événement Anthropic non pris en charge.".into()),
        }if self.blocks.values().map(|block|block.arguments.len()).sum::<usize>()>16*1024*1024{return Err("Appels d'outils trop volumineux.".into())}Ok(out)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn builds_messages_with_tools_and_preserves_results(){
        let input=json!({"model":"claude","input":[{"role":"user","content":"Read"},{"type":"function_call","name":"read","call_id":"c1","arguments":"{\"path\":\"a\"}"},{"type":"function_call_output","call_id":"c1","output":"result"}],"instructions":"Be concise","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}],"max_output_tokens":100});
        let(chat,_)=crate::responses_adapter::request(&input).unwrap();let result=request(&chat).unwrap();
        assert_eq!(result["system"][0]["text"],"Be concise");assert_eq!(result["max_tokens"],100);
        assert_eq!(result["messages"][1]["content"][0]["type"],"tool_use");assert_eq!(result["messages"][1]["content"][0]["input"]["path"],"a");
        assert_eq!(result["messages"][2]["content"][0]["type"],"tool_result");assert_eq!(result["messages"][2]["content"][0]["tool_use_id"],"c1");
        assert_eq!(result["tools"][0]["name"],"read");
    }
    #[test]
    fn translates_non_streaming_output_and_usage(){
        let (_,mapping)=crate::responses_adapter::request(&json!({"model":"m","input":"x","tools":[{"type":"function","name":"read"}]})).unwrap();
        let message=json!({"content":[{"type":"text","text":"Okay"},{"type":"tool_use","id":"c1","name":"read","input":{"path":"a"}}],"stop_reason":"tool_use","usage":{"input_tokens":7,"output_tokens":4}});
        let chat=non_stream(&message).unwrap();let response=crate::responses_adapter::non_stream(&chat,"anthropic/m",&mapping).unwrap();
        assert_eq!(response["output"][0]["content"][0]["text"],"Okay");assert_eq!(response["output"][1]["call_id"],"c1");assert_eq!(response["usage"]["total_tokens"],11);
        assert!(non_stream(&json!({"content":[{"type":"thinking","thinking":"hidden"}],"stop_reason":"end_turn"})).is_err());
    }
    #[test]
    fn bridges_fragmented_events_to_responses_tool_calls(){
        let (_,mapping)=crate::responses_adapter::request(&json!({"model":"m","input":"x","tools":[{"type":"namespace","name":"work","tools":[{"type":"function","name":"read"}]}]})).unwrap();
        let events=[
            json!({"type":"message_start","message":{"usage":{"input_tokens":8}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Bonjour"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"c1","name":"work__read","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ];
        let encoded=events.iter().map(|event|format!("data: {event}\r\n\r\n")).collect::<String>();
        let mut bridge=StreamBridge::new();let mut response=crate::responses_adapter::StreamAdapter::new("anthropic/m".into(),mapping);let mut output=response.start();
        for part in encoded.as_bytes().chunks(11){for chunk in bridge.ingest(part).unwrap(){output.extend(response.ingest(&chunk).unwrap())}}
        let records:Vec<Value>=output.iter().map(|event|serde_json::from_str(std::str::from_utf8(event).unwrap().lines().find_map(|line|line.strip_prefix("data: ")).unwrap()).unwrap()).collect();
        let completed=records.last().unwrap();assert_eq!(completed["type"],"response.completed");
        assert_eq!(completed["response"]["output"][1]["namespace"],"work");assert_eq!(completed["response"]["output"][1]["arguments"],"{\"path\":\"a\"}");
        assert_eq!(completed["response"]["usage"]["total_tokens"],12);
    }
    #[test]
    fn refuses_invalid_tool_json_and_unfinished_streams(){
        let events=[json!({"type":"message_start","message":{"usage":{}}}),json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c","name":"read","input":{}}}),json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}),json!({"type":"content_block_stop","index":0})];
        let mut bridge=StreamBridge::new();let input=events.iter().map(|event|format!("data: {event}\n\n")).collect::<String>();assert!(bridge.ingest(input.as_bytes()).is_err());
        let mut bridge=StreamBridge::new();assert!(bridge.ingest(b"data: {\"type\":\"message_stop\"}\n\n").is_err());
    }
}
