//! Explicit conversion of the supported Responses subset to Chat Completions.
//! Unsupported stateful or built-in tools are rejected instead of discarded.
use std::{collections::BTreeMap, time::{SystemTime, UNIX_EPOCH}};
use bytes::Bytes;
use serde_json::{json, Map, Value};

fn content(value: &Value) -> Result<Value, String> {
    if let Some(text) = value.as_str() { return Ok(json!(text)); }
    let parts = value.as_array().ok_or("Contenu de message non pris en charge.")?;
    let mut converted = Vec::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text" | "text") => {
                let text = part.get("text").and_then(Value::as_str).ok_or("Texte de message invalide.")?;
                converted.push(json!({"type":"text","text":text}));
            }
            Some("input_image") => {
                let url = part.get("image_url").and_then(Value::as_str).ok_or("Image sans URL : ce format n'est pas pris en charge.")?;
                let mut image = json!({"url":url});
                if let Some(detail) = part.get("detail") { image["detail"] = detail.clone(); }
                converted.push(json!({"type":"image_url","image_url":image}));
            }
            _ => return Err("Ce type de contenu Responses n'est pas encore pris en charge.".into()),
        }
    }
    Ok(Value::Array(converted))
}

#[derive(Clone, Debug)]
pub struct ToolSpec { name:String, namespace:Option<String> }
pub type ToolMap = BTreeMap<String, ToolSpec>;

fn chat_name(namespace: Option<&str>, name: &str) -> String {
    namespace.map_or_else(||name.to_string(),|namespace|format!("{namespace}__{name}"))
}

fn convert_tools(input:&Value) -> Result<(Vec<Value>,ToolMap),String> {
    let mut result=Vec::new(); let mut mapping=ToolMap::new();
    let Some(tools)=input.get("tools").filter(|value|!value.is_null()) else { return Ok((result,mapping)) };
    let tools=tools.as_array().ok_or("Liste d'outils invalide.")?;
    let mut flattened:Vec<(Option<&str>,&Value)>=Vec::new();
    for tool in tools {
        if tool.get("type").and_then(Value::as_str)==Some("namespace") {
            let namespace=tool.get("name").and_then(Value::as_str).filter(|value|!value.is_empty()).ok_or("Espace de noms d'outil invalide.")?;
            let members=tool.get("tools").and_then(Value::as_array).ok_or("Espace de noms sans outils.")?;
            for member in members { flattened.push((Some(namespace),member)); }
        } else { flattened.push((None,tool)); }
    }
    if flattened.len()>256 { return Err("Trop d'outils déclarés.".into()); }
    for (namespace,tool) in flattened {
        if tool.get("type").and_then(Value::as_str)!=Some("function") { return Err(format!("Type d'outil non pris en charge : {}.",tool.get("type").and_then(Value::as_str).unwrap_or("inconnu"))); }
        let name=tool.get("name").and_then(Value::as_str).filter(|value|!value.is_empty()).ok_or("Outil sans nom.")?;
        if mapping.values().any(|spec|spec.name==name && spec.namespace.as_deref()==namespace) { return Err("Nom d'outil en double.".into()); }
        let candidate=chat_name(namespace,name);
        let valid=candidate.len()<=64 && candidate.bytes().all(|byte|byte.is_ascii_alphanumeric()||byte==b'_'||byte==b'-');
        let mut alias=if valid && !mapping.contains_key(&candidate) {candidate} else {format!("nivra_tool_{}",result.len())};
        while mapping.contains_key(&alias) { alias.push('_'); }
        let mut function=Map::new();
        for key in ["description","parameters","strict"] { if let Some(value)=tool.get(key) {function.insert(key.into(),value.clone());} }
        if let Some(namespace)=namespace {
            let description=function.get("description").and_then(Value::as_str).unwrap_or("");
            function.insert("description".into(),json!(format!("{namespace}.{name}: {description}")));
        }
        function.insert("name".into(),json!(alias));
        mapping.insert(alias,ToolSpec{name:name.into(),namespace:namespace.map(str::to_owned)});
        result.push(json!({"type":"function","function":function}));
    }
    Ok((result,mapping))
}

fn function_call(item: &Value, mapping:&ToolMap) -> Result<Value, String> {
    let id = item.get("call_id").and_then(Value::as_str).ok_or("Appel d'outil sans call_id.")?;
    let name = item.get("name").and_then(Value::as_str).ok_or("Appel d'outil sans nom.")?;
    let arguments = item.get("arguments").and_then(Value::as_str).ok_or("Arguments d'outil invalides.")?;
    let namespace=item.get("namespace").and_then(Value::as_str);
    let alias=mapping.iter().find(|(_,spec)|spec.name==name && spec.namespace.as_deref()==namespace).map(|(alias,_)|alias.as_str()).ok_or("Appel d'outil absent des outils déclarés.")?;
    Ok(json!({"id":id,"type":"function","function":{"name":alias,"arguments":arguments}}))
}

pub fn request(input: &Value) -> Result<(Value, ToolMap), String> {
    let source = input.as_object().ok_or("Requête Responses invalide.")?;
    for key in ["previous_response_id", "conversation", "background", "prompt", "truncation"] {
        if source.get(key).is_some_and(|value| !value.is_null() && value != false && value != "disabled") {
            return Err(format!("L'option {key} n'est pas prise en charge par cette traduction."));
        }
    }
    if source.get("store").and_then(Value::as_bool) == Some(true) { return Err("Le stockage de réponses n'est pas pris en charge par cette traduction.".into()); }
    if source.get("include").and_then(Value::as_array).is_some_and(|values| !values.is_empty()) { return Err("Les inclusions supplémentaires ne sont pas prises en charge par cette traduction.".into()); }
    let (tools, mapping)=convert_tools(input)?;
    let model = input.get("model").and_then(Value::as_str).ok_or("Modèle manquant.")?;
    let mut messages = Vec::new();
    if let Some(instructions) = input.get("instructions").filter(|value| !value.is_null()) {
        messages.push(json!({"role":"system","content":instructions.as_str().ok_or("Instructions invalides.")?}));
    }
    let mut previous_call = false;
    match input.get("input") {
        Some(Value::String(text)) => messages.push(json!({"role":"user","content":text})),
        Some(Value::Array(items)) => for item in items {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("message");
            match kind {
                "message" => {
                    let role = item.get("role").and_then(Value::as_str).ok_or("Rôle de message manquant.")?;
                    if !matches!(role, "user" | "assistant" | "system" | "developer") { return Err("Rôle de message non pris en charge.".into()); }
                    let text = content(item.get("content").ok_or("Contenu de message manquant.")?)?;
                    messages.push(json!({"role":role,"content":text})); previous_call = false;
                }
                "function_call" => {
                    let call = function_call(item,&mapping)?;
                    if previous_call { messages.last_mut().unwrap()["tool_calls"].as_array_mut().unwrap().push(call); }
                    else { messages.push(json!({"role":"assistant","content":null,"tool_calls":[call]})); }
                    previous_call = true;
                }
                "function_call_output" => {
                    let id = item.get("call_id").and_then(Value::as_str).ok_or("Résultat d'outil sans call_id.")?;
                    let output = item.get("output").and_then(Value::as_str).ok_or("Résultat d'outil non textuel non pris en charge.")?;
                    messages.push(json!({"role":"tool","tool_call_id":id,"content":output})); previous_call = false;
                }
                _ => return Err(format!("Élément Responses non pris en charge : {kind}.")),
            }
        },
        _ => return Err("Le champ input doit contenir du texte ou une liste de messages.".into()),
    }
    let mut result = json!({"model":model,"messages":messages,"stream":input.get("stream").and_then(Value::as_bool).unwrap_or(false)});
    for key in ["temperature", "top_p", "parallel_tool_calls", "user"] { if let Some(value) = source.get(key) { result[key] = value.clone(); } }
    if let Some(value) = source.get("max_output_tokens") { result["max_tokens"] = value.clone(); }
    if let Some(reasoning) = input.get("reasoning").filter(|value| !value.is_null()) {
        if let Some(effort) = reasoning.get("effort") { result["reasoning_effort"] = effort.clone(); }
    }
    if input.get("tools").is_some_and(|value|!value.is_null()) { result["tools"] = Value::Array(tools); }
    if let Some(choice) = input.get("tool_choice").filter(|value| !value.is_null()) {
        result["tool_choice"] = if choice.is_string() { choice.clone() }
        else if choice.get("type").and_then(Value::as_str) == Some("function") {
            let name=choice.get("name").and_then(Value::as_str).ok_or("Choix d'outil invalide.")?;
            let namespace=choice.get("namespace").and_then(Value::as_str);
            let alias=mapping.iter().find(|(_,spec)|spec.name==name && spec.namespace.as_deref()==namespace).map(|(alias,_)|alias).ok_or("Choix d'outil non déclaré.")?;
            json!({"type":"function","function":{"name":alias}})
        } else { return Err("Choix d'outil non pris en charge.".into()); };
    }
    if let Some(format) = input.pointer("/text/format") {
        match format.get("type").and_then(Value::as_str) {
            Some("text") => {},
            Some("json_object") => result["response_format"] = json!({"type":"json_object"}),
            Some("json_schema") => {
                let mut schema = Map::new();
                for key in ["name", "schema", "description", "strict"] { if let Some(value) = format.get(key) { schema.insert(key.into(), value.clone()); } }
                result["response_format"] = json!({"type":"json_schema","json_schema":schema});
            }
            _ => return Err("Format de sortie non pris en charge.".into()),
        }
    }
    Ok((result,mapping))
}

fn created_at() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn new_id(prefix: &str) -> String { format!("{prefix}_{}", uuid::Uuid::new_v4().simple()) }
fn usage(value: &Value) -> Value {
    if value.is_null() { return Value::Null; }
    let input = value.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = value.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    json!({"input_tokens":input,"input_tokens_details":{"cached_tokens":value.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64).unwrap_or(0)},"output_tokens":output,"output_tokens_details":{"reasoning_tokens":value.pointer("/completion_tokens_details/reasoning_tokens").and_then(Value::as_u64).unwrap_or(0)},"total_tokens":value.get("total_tokens").and_then(Value::as_u64).unwrap_or(input+output)})
}

fn output_items(message: &Value, mapping:&ToolMap) -> Result<Vec<Value>, String> {
    let mut output = Vec::new();
    if message.get("content").is_some_and(|value| !value.is_null() && !value.is_string()) { return Err("Contenu du fournisseur non pris en charge.".into()); }
    if message.get("function_call").is_some_and(|value| !value.is_null()) { return Err("Ancien format d'appel de fonction non pris en charge.".into()); }
    if ["reasoning_content","reasoning","reasoning_details"].iter().any(|key|message.get(key).is_some_and(|value|!value.is_null()&&value!= ""&&value!=&json!([]))) { return Err("La sortie de raisonnement de ce fournisseur n'est pas encore prise en charge.".into()); }
    if let Some(text) = message.get("content").and_then(Value::as_str).filter(|text| !text.is_empty()) {
        output.push(json!({"id":new_id("msg"),"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":text,"annotations":[]}]}));
    }
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str).filter(|text| !text.is_empty()) {
        output.push(json!({"id":new_id("msg"),"type":"message","status":"completed","role":"assistant","content":[{"type":"refusal","refusal":refusal}]}));
    }
    if let Some(calls) = message.get("tool_calls").filter(|v| !v.is_null()) {
        for call in calls.as_array().ok_or("Appels d'outils invalides.")? {
            if call.get("type").and_then(Value::as_str) != Some("function") { return Err("Type d'appel d'outil non pris en charge.".into()); }
            let call_id = call.get("id").and_then(Value::as_str).ok_or("Appel d'outil sans identifiant.")?;
            let alias = call.pointer("/function/name").and_then(Value::as_str).ok_or("Appel d'outil sans nom.")?;
            let spec=mapping.get(alias).ok_or("Le fournisseur a appelé un outil non déclaré.")?;
            let arguments = call.pointer("/function/arguments").and_then(Value::as_str).ok_or("Arguments d'outil invalides.")?;
            let mut item=json!({"id":new_id("fc"),"type":"function_call","status":"completed","call_id":call_id,"name":spec.name,"arguments":arguments});
            if let Some(namespace)=&spec.namespace { item["namespace"]=json!(namespace); }
            output.push(item);
        }
    }
    Ok(output)
}

fn response_object(id: &str, model: &str, created: u64, status: &str, output: Vec<Value>, token_usage: Value, reason: Option<&str>) -> Value {
    json!({"id":id,"object":"response","created_at":created,"status":status,"model":model,"output":output,"usage":token_usage,"error":null,"incomplete_details":reason.map(|reason|json!({"reason":reason}))})
}

pub fn non_stream(value: &Value, model: &str, mapping:&ToolMap) -> Result<Value, String> {
    let choice = value.get("choices").and_then(Value::as_array).and_then(|choices| choices.first()).ok_or("Réponse du fournisseur sans choix.")?;
    let message = choice.get("message").ok_or("Réponse du fournisseur sans message.")?;
    let reason = match choice.get("finish_reason").and_then(Value::as_str) { Some("length") => Some("max_output_tokens"), Some("content_filter") => Some("content_filter"), Some("stop" | "tool_calls" | "function_call") => None, _ => return Err("État final du fournisseur non pris en charge.".into()) };
    Ok(response_object(&new_id("resp"), model, created_at(), if reason.is_some(){"incomplete"}else{"completed"}, output_items(message,mapping)?, usage(value.get("usage").unwrap_or(&Value::Null)), reason))
}

#[derive(Default)]
struct Tool { call_id: String, name: String, arguments: String }
pub struct StreamAdapter {
    id: String, message_id: String, model: String, created: u64, sequence: u64,
    pending: Vec<u8>, data_lines: Vec<String>, text: String, tools: BTreeMap<u64, Tool>,
    usage: Value, finish_reason: Option<String>, started_text: bool, done: bool, mapping:ToolMap,
}

impl StreamAdapter {
    pub fn new(model: String, mapping:ToolMap) -> Self { Self { id:new_id("resp"), message_id:new_id("msg"), model, created:created_at(), sequence:0, pending:Vec::new(), data_lines:Vec::new(), text:String::new(), tools:BTreeMap::new(), usage:Value::Null, finish_reason:None, started_text:false, done:false, mapping } }
    fn event(&mut self, kind: &str, mut data: Value) -> Bytes {
        data["type"] = json!(kind); data["sequence_number"] = json!(self.sequence); self.sequence += 1;
        Bytes::from(format!("event: {kind}\ndata: {data}\n\n"))
    }
    pub fn start(&mut self) -> Vec<Bytes> {
        let response = response_object(&self.id,&self.model,self.created,"in_progress",vec![],Value::Null,None);
        vec![self.event("response.created",json!({"response":response})),self.event("response.in_progress",json!({"response":response}))]
    }
    pub fn ingest(&mut self, chunk: &[u8]) -> Result<Vec<Bytes>, String> {
        if self.done { return Ok(vec![]); }
        self.pending.extend_from_slice(chunk);
        if self.pending.len() > 2*1024*1024 { return Err("Événement du fournisseur trop volumineux.".into()); }
        let mut events = Vec::new();
        while let Some(end) = self.pending.iter().position(|byte| *byte==b'\n') {
            let mut line: Vec<u8> = self.pending.drain(..=end).collect(); line.pop(); if line.last()==Some(&b'\r') { line.pop(); }
            let line = String::from_utf8(line).map_err(|_| "Flux du fournisseur invalide.")?;
            if line.is_empty() {
                if !self.data_lines.is_empty() { let data=std::mem::take(&mut self.data_lines).join("\n"); events.extend(self.data(&data)?); }
                if self.done { break; }
            } else if let Some(data)=line.strip_prefix("data:") {
                let data = data.strip_prefix(' ').unwrap_or(data);
                if self.data_lines.len() >= 8192 || data.len()+self.data_lines.iter().map(String::len).sum::<usize>() > 2*1024*1024 { return Err("Événement du fournisseur trop volumineux.".into()); }
                self.data_lines.push(data.to_owned());
            }
        }
        Ok(events)
    }
    fn data(&mut self, data: &str) -> Result<Vec<Bytes>, String> {
        if data=="[DONE]" { return self.complete(); }
        let chunk: Value = serde_json::from_str(data).map_err(|_| "Événement JSON invalide.")?;
        if chunk.get("error").is_some() { return Err("Le fournisseur a interrompu la réponse.".into()); }
        if let Some(usage) = chunk.get("usage").filter(|usage| !usage.is_null()) { self.usage=usage.clone(); }
        let Some(choice)=chunk.get("choices").and_then(Value::as_array).and_then(|choices| choices.first()) else { return Ok(vec![]) };
        if let Some(reason)=choice.get("finish_reason").and_then(Value::as_str) { self.finish_reason=Some(reason.to_owned()); }
        let delta=&choice["delta"];
        if delta.get("refusal").and_then(Value::as_str).is_some() { return Err("Les refus en streaming ne sont pas encore pris en charge.".into()); }
        if delta.get("content").is_some_and(|value| !value.is_null() && !value.is_string()) { return Err("Contenu du flux non pris en charge.".into()); }
        if delta.get("function_call").is_some_and(|value| !value.is_null()) { return Err("Ancien format d'appel de fonction non pris en charge.".into()); }
        if ["reasoning_content","reasoning","reasoning_details"].iter().any(|key|delta.get(key).is_some_and(|value|!value.is_null()&&value!=""&&value!=&json!([]))) { return Err("La sortie de raisonnement de ce fournisseur n'est pas encore prise en charge.".into()); }
        let mut events=Vec::new();
        if let Some(text)=delta.get("content").and_then(Value::as_str).filter(|text| !text.is_empty()) {
            if self.text.len()+text.len()>8*1024*1024 { return Err("Réponse trop volumineuse.".into()); }
            if !self.started_text {
                self.started_text=true;
                events.push(self.event("response.output_item.added",json!({"output_index":0,"item":{"id":self.message_id,"type":"message","status":"in_progress","role":"assistant","content":[]}})));
                events.push(self.event("response.content_part.added",json!({"item_id":self.message_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})));
            }
            self.text.push_str(text);
            events.push(self.event("response.output_text.delta",json!({"item_id":self.message_id,"output_index":0,"content_index":0,"delta":text,"logprobs":[]})));
        }
        if let Some(calls)=delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                if call.get("type").and_then(Value::as_str).is_some_and(|kind| kind != "function") { return Err("Type d'appel d'outil non pris en charge.".into()); }
                let index=call.get("index").and_then(Value::as_u64).ok_or("Index d'outil manquant.")?;
                if index>127 { return Err("Trop d'appels d'outils.".into()); }
                let tool=self.tools.entry(index).or_default();
                if let Some(id)=call.get("id").and_then(Value::as_str) { tool.call_id.push_str(id); }
                if let Some(name)=call.pointer("/function/name").and_then(Value::as_str) { tool.name.push_str(name); }
                if let Some(args)=call.pointer("/function/arguments").and_then(Value::as_str) { tool.arguments.push_str(args); }
                if tool.call_id.len()>4096 || tool.name.len()>4096 || tool.arguments.len()>8*1024*1024 { return Err("Appel d'outil trop volumineux.".into()); }
            }
        }
        Ok(events)
    }
    fn complete(&mut self) -> Result<Vec<Bytes>, String> {
        let reason=self.finish_reason.clone().ok_or("Flux terminé sans état final.")?;
        if !matches!(reason.as_str(), "stop" | "tool_calls" | "function_call" | "length" | "content_filter") { return Err("État final du fournisseur non pris en charge.".into()); }
        let mut events=Vec::new(); let mut output=Vec::new();
        if self.started_text {
            let text=self.text.clone();
            let part=json!({"type":"output_text","text":text,"annotations":[]});
            let item=json!({"id":self.message_id,"type":"message","status":"completed","role":"assistant","content":[part]});
            events.push(self.event("response.output_text.done",json!({"item_id":self.message_id,"output_index":0,"content_index":0,"text":text,"logprobs":[]})));
            events.push(self.event("response.content_part.done",json!({"item_id":self.message_id,"output_index":0,"content_index":0,"part":part})));
            events.push(self.event("response.output_item.done",json!({"output_index":0,"item":item})));
            output.push(item);
        }
        for (_,tool) in std::mem::take(&mut self.tools) {
            if tool.call_id.is_empty() || tool.name.is_empty() { return Err("Appel d'outil incomplet.".into()); }
            let spec=self.mapping.get(&tool.name).cloned().ok_or("Le fournisseur a appelé un outil non déclaré.")?;
            let index=output.len(); let id=new_id("fc");
            let mut item=json!({"id":id,"type":"function_call","status":"in_progress","call_id":tool.call_id,"name":spec.name,"arguments":""});
            if let Some(namespace)=&spec.namespace { item["namespace"]=json!(namespace); }
            events.push(self.event("response.output_item.added",json!({"output_index":index,"item":item})));
            if !tool.arguments.is_empty() { events.push(self.event("response.function_call_arguments.delta",json!({"item_id":id,"output_index":index,"delta":tool.arguments}))); }
            events.push(self.event("response.function_call_arguments.done",json!({"item_id":id,"output_index":index,"arguments":tool.arguments,"name":spec.name})));
            item["status"]=json!("completed"); item["arguments"]=json!(tool.arguments);
            events.push(self.event("response.output_item.done",json!({"output_index":index,"item":item})));
            output.push(item);
        }
        let incomplete=match reason.as_str(){"length"=>Some("max_output_tokens"),"content_filter"=>Some("content_filter"),_=>None};
        let response=response_object(&self.id,&self.model,self.created,if incomplete.is_some(){"incomplete"}else{"completed"},output,usage(&self.usage),incomplete);
        events.push(self.event(if incomplete.is_some(){"response.incomplete"}else{"response.completed"},json!({"response":response})));
        self.done=true; Ok(events)
    }
    pub fn fail(&mut self, message: &str) -> Vec<Bytes> {
        if self.done { return vec![]; } self.done=true;
        let mut response=response_object(&self.id,&self.model,self.created,"failed",vec![],Value::Null,None);
        response["error"]=json!({"code":"upstream_error","message":message});
        vec![self.event("response.failed",json!({"response":response}))]
    }
    pub fn is_done(&self) -> bool { self.done }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translates_messages_images_and_tool_roundtrip() {
        let input = json!({"model":"sample", "instructions":"system", "stream":true, "max_output_tokens":123,
            "input":[{"role":"user","content":[{"type":"input_text","text":"Hi"},{"type":"input_image","image_url":"https://example.com/a.png"}]},
                {"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_1","output":"ok"}],
            "tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]});
        let (result,_) = request(&input).unwrap();
        assert_eq!(result["messages"][0]["role"], "system");
        assert_eq!(result["messages"][1]["content"][1]["image_url"]["url"], "https://example.com/a.png");
        assert_eq!(result["messages"][2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(result["messages"][3]["tool_call_id"], "call_1");
        assert_eq!(result["tools"][0]["function"]["name"], "read");
        assert_eq!(result["max_tokens"], 123);
    }
    #[test]
    fn refuses_unsupported_state_and_tools() {
        assert!(request(&json!({"model":"m","input":"x","previous_response_id":"old"})).is_err());
        assert!(request(&json!({"model":"m","input":"x","tools":[{"type":"web_search"}]})).is_err());
    }
    #[test]
    fn namespaces_roundtrip_without_collisions() {
        let input=json!({"model":"m","input":[{"type":"function_call","namespace":"work","name":"read","call_id":"c1","arguments":"{}"},{"type":"function_call_output","call_id":"c1","output":"ok"}],"tools":[{"type":"namespace","name":"work","description":"workspace","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]},{"type":"function","name":"work__read","parameters":{"type":"object"}}]});
        let (chat,mapping)=request(&input).unwrap();
        let first=chat["tools"][0]["function"]["name"].as_str().unwrap();
        let second=chat["tools"][1]["function"]["name"].as_str().unwrap();
        assert_ne!(first,second);
        assert_eq!(chat["messages"][0]["tool_calls"][0]["function"]["name"],first);
        let answer=json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"call_1","type":"function","function":{"name":first,"arguments":"{}"}}]}}]});
        let response=non_stream(&answer,"m",&mapping).unwrap();
        assert_eq!(response["output"][0]["name"],"read");
        assert_eq!(response["output"][0]["namespace"],"work");
        let mut stream=StreamAdapter::new("m".into(),mapping);
        let payload=format!("data: {}\n\ndata: [DONE]\n\n",json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":first,"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}));
        let events=stream.ingest(payload.as_bytes()).unwrap();
        let output:Vec<Value>=events.iter().map(|event|serde_json::from_str(std::str::from_utf8(event).unwrap().lines().find_map(|line|line.strip_prefix("data: ")).unwrap()).unwrap()).collect();
        for kind in ["response.output_item.added","response.output_item.done"] {
            let event=output.iter().find(|event|event["type"]==kind).unwrap();
            assert_eq!(event["item"]["name"],"read");
            assert_eq!(event["item"]["namespace"],"work");
        }
    }
    #[test]
    fn converts_non_streaming_tool_response_and_usage() {
        let (_,mapping)=request(&json!({"model":"m","input":"x","tools":[{"type":"function","name":"read"}]})).unwrap();
        let result=non_stream(&json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"Je vérifie.","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{}"}}]}}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}),"provider/model",&mapping).unwrap();
        assert_eq!(result["model"],"provider/model");
        assert_eq!(result["output"][0]["content"][0]["text"],"Je vérifie.");
        assert_eq!(result["output"][1]["call_id"],"call_1");
        assert_eq!(result["usage"]["total_tokens"],8);
    }
    #[test]
    fn streams_split_utf8_and_fragmented_tool_calls() {
        let chunks=[
            json!({"choices":[{"delta":{"content":"Été"},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]},"finish_reason":"tool_calls"}]}),
        ];
        let input=format!("{}data: [DONE]\r\n\r\n",chunks.iter().map(|chunk|format!("data: {chunk}\r\n\r\n")).collect::<String>());
        let (_,mapping)=request(&json!({"model":"m","input":"x","tools":[{"type":"function","name":"read"}]})).unwrap();
        let mut adapter=StreamAdapter::new("provider/model".into(),mapping);
        let mut events=adapter.start();
        for part in input.as_bytes().chunks(7) { events.extend(adapter.ingest(part).unwrap()); }
        assert!(adapter.is_done());
        let records: Vec<Value> = events.iter().map(|event| {
            let text=std::str::from_utf8(event).unwrap();
            serde_json::from_str(text.lines().find_map(|line|line.strip_prefix("data: ")).unwrap()).unwrap()
        }).collect();
        for (index,record) in records.iter().enumerate() { assert_eq!(record["sequence_number"],index); }
        assert!(records.iter().any(|value|value["type"]=="response.output_text.delta" && value["delta"]=="Été"));
        let final_response=&records.last().unwrap()["response"];
        assert_eq!(records.last().unwrap()["type"],"response.completed");
        assert_eq!(final_response["output"][1]["call_id"],"call_1");
        assert_eq!(final_response["output"][1]["arguments"],"{\"path\":\"a\"}");
    }
    #[test]
    fn reports_unfinished_or_unsupported_streams_as_failures() {
        let mut adapter=StreamAdapter::new("model".into(),ToolMap::new());
        adapter.start();
        assert!(adapter.ingest(b"data: [DONE]\n\n").is_err());
        let failure=adapter.fail("Flux incomplet");
        let text=std::str::from_utf8(&failure[0]).unwrap();
        assert!(text.contains("event: response.failed"));
        assert!(!text.contains("response.completed"));
        let mut adapter=StreamAdapter::new("model".into(),ToolMap::new());
        let chunk=b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"private\"}}]}\n\n";
        assert!(adapter.ingest(chunk).is_err());
        for chunk in [b"data: {\"choices\":[{\"delta\":{\"reasoning\":\"private\"}}]}\n\n".as_slice(),b"data: {\"choices\":[{\"delta\":{\"reasoning_details\":[{\"data\":\"opaque\"}]}}]}\n\n".as_slice()]{let mut adapter=StreamAdapter::new("model".into(),ToolMap::new());assert!(adapter.ingest(chunk).is_err());}
        let answer=json!({"choices":[{"message":{"role":"assistant","content":"x","reasoning_details":[{"data":"opaque"}]},"finish_reason":"stop"}]});assert!(non_stream(&answer,"model",&ToolMap::new()).is_err());
    }
}
