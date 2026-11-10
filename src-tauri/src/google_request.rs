//! Translation helpers for Google GenerateContent requests and signed tool turns.
use serde_json::{json,Value};
use base64::{engine::general_purpose::STANDARD,Engine};

/// Convert user text and inlined image data without fetching external URLs.
pub fn content_parts(content:&Value,role:&str)->Result<Vec<Value>,String>{
    if let Some(text)=content.as_str().filter(|s|!s.trim().is_empty()){return Ok(vec![json!({"text":text})])}
    let input=content.as_array().filter(|parts|!parts.is_empty()&&parts.len()<=128).ok_or("Contenu Antigravity invalide.")?;
    let mut parts=Vec::new();
    for item in input{
        let part=item.as_object().ok_or("Fragment Antigravity invalide.")?;
        match part.get("type").and_then(Value::as_str){
            Some("text") if part.keys().all(|k|matches!(k.as_str(),"type"|"text"))=>{let text=part.get("text").and_then(Value::as_str).ok_or("Texte invalide.")?;parts.push(json!({"text":text}));},
            Some("image_url") if role=="user"&&part.keys().all(|k|matches!(k.as_str(),"type"|"image_url"))=>{
                let image=part.get("image_url").and_then(Value::as_object).filter(|v|v.keys().all(|k|matches!(k.as_str(),"url"|"detail"))).ok_or("Image invalide.")?;
                if image.get("detail").is_some_and(|v|!matches!(v.as_str(),Some("auto"))){return Err("Le détail de l’image n’est pas pris en charge par Antigravity.".into())}
                let url=image.get("url").and_then(Value::as_str).ok_or("Image absente.")?;let (metadata,data)=url.split_once(',').ok_or("Utilise une image intégrée en base64.")?;
                let mime=metadata.strip_prefix("data:").and_then(|v|v.strip_suffix(";base64")).filter(|m|matches!(*m,"image/png"|"image/jpeg"|"image/webp"|"image/gif")).ok_or("Format d’image non pris en charge.")?;
                if data.is_empty()||data.len()>8*1024*1024||STANDARD.decode(data).is_err(){return Err("Image base64 invalide ou trop volumineuse.".into())}
                parts.push(json!({"inlineData":{"mimeType":mime,"data":data}}));
            },
            _=>return Err("Fragment Antigravity non pris en charge.".into())
        }
    }
    if parts.is_empty(){return Err("Contenu Antigravity vide.".into())}Ok(parts)
}

/// Translate explicitly supplied sampling controls without changing model defaults.
pub fn generation_options(source:&Value,request:&mut Value)->Result<(),String>{
    let mut config=serde_json::Map::new();
    for (key,target,max) in [("temperature","temperature",2.0),("top_p","topP",1.0)]{
        if let Some(value)=source.get(key).filter(|v|!v.is_null()){
            let number=value.as_f64().filter(|n|n.is_finite()&&*n>=0.0&&*n<=max).ok_or("Paramètre de génération invalide.")?;
            config.insert(target.into(),json!(number));
        }
    }
    if let Some(value)=source.get("stop").filter(|v|!v.is_null()){
        let items=if let Some(text)=value.as_str(){vec![text]}else{value.as_array().ok_or("Séquences d’arrêt invalides.")?.iter().map(|v|v.as_str().ok_or("Séquence d’arrêt invalide.")).collect::<Result<Vec<_>,_>>()?};
        if items.is_empty()||items.len()>4||items.iter().any(|s|s.is_empty()||s.len()>1000){return Err("Séquences d’arrêt invalides.".into())}
        config.insert("stopSequences".into(),json!(items));
    }
    if !config.is_empty(){request["generationConfig"]=Value::Object(config)}
    Ok(())
}

/// Convert the initial, text-only portion of a Chat request without discarding fields.
/// Tool-call history is intentionally refused until thought signatures can be preserved.
pub fn request(chat:&Value)->Result<Value,String>{
    let object=chat.as_object().ok_or("Requête invalide.")?;
    let supported=["model","messages","stream","max_tokens","temperature","top_p","tools"];
    if object.iter().any(|(key,value)|!supported.contains(&key.as_str())&&!value.is_null()){return Err("Option Google non prise en charge.".into())}
    if object.get("model").and_then(Value::as_str).filter(|s|!s.is_empty()).is_none(){return Err("Modèle manquant.".into())}
    let history=object.get("messages").and_then(Value::as_array).ok_or("Messages manquants.")?;
    let mut contents:Vec<Value>=Vec::new();let mut system=Vec::new();
    for message in history {
        let item=message.as_object().ok_or("Message invalide.")?;
        if item.iter().any(|(key,value)|!matches!(key.as_str(),"role"|"content")&&!value.is_null()){return Err("Historique d'outils ou métadonnées non pris en charge.".into())}
        let role=item.get("role").and_then(Value::as_str).ok_or("Rôle manquant.")?;
        let content=item.get("content").ok_or("Contenu manquant.")?;
        if matches!(role,"system"|"developer") {let text=content.as_str().filter(|s|!s.trim().is_empty()).ok_or("Instructions texte requises.")?;if !contents.is_empty(){return Err("Instructions au milieu de l'historique non prises en charge.".into())}system.push(json!({"text":text}));continue}
        let parts=content_parts(content,role)?;
        let role=match role{"user"=>"user","assistant"=>"model",_=>return Err("Rôle non pris en charge.".into())};
        if let Some(last)=contents.last_mut().filter(|last|last["role"]==role){last["parts"].as_array_mut().unwrap().extend(parts);}else{contents.push(json!({"role":role,"parts":parts}));}
    }
    if contents.is_empty(){return Err("Conversation vide.".into())}
    let mut result=json!({"contents":contents});
    if !system.is_empty(){result["systemInstruction"]=json!({"parts":system})}
    let mut generation=serde_json::Map::new();
    for (source,target) in [("max_tokens","maxOutputTokens"),("temperature","temperature"),("top_p","topP")] {if let Some(value)=object.get(source).filter(|value|!value.is_null()){if !value.is_number(){return Err("Paramètre de génération invalide.".into())}generation.insert(target.into(),value.clone());}}
    if !generation.is_empty(){result["generationConfig"]=Value::Object(generation)}
    if let Some(tools)=object.get("tools").filter(|v|!v.is_null()){
        let mut declarations=Vec::new();
        let tools=tools.as_array().filter(|tools|tools.len()<=128).ok_or("Outils invalides.")?;
        let mut names=std::collections::BTreeSet::new();
        for tool in tools {
            if tool.as_object().is_none_or(|object|object.keys().any(|key|!matches!(key.as_str(),"type"|"function"))){return Err("Option d’outil non prise en charge.".into())}
            if tool.get("type").and_then(Value::as_str)!=Some("function"){return Err("Outil non pris en charge.".into())}
            let f=tool.get("function").and_then(Value::as_object).ok_or("Fonction invalide.")?;
            if f.keys().any(|key|!matches!(key.as_str(),"name"|"description"|"parameters")){return Err("Option de fonction non prise en charge.".into())}
            let name=f.get("name").and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=128&&s.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-'))).ok_or("Nom de fonction invalide.")?;
            if !names.insert(name.to_owned()){return Err("Nom d’outil répété.".into())}
            let mut declaration=json!({"name":name});
            if let Some(description)=f.get("description"){if !description.is_string(){return Err("Description invalide.".into())}declaration["description"]=description.clone()}
            if let Some(schema)=f.get("parameters"){if !schema.is_object(){return Err("Schéma de fonction invalide.".into())}declaration["parametersJsonSchema"]=schema.clone()}
            declarations.push(declaration);
        }
        if !declarations.is_empty(){result["tools"]=json!([{"functionDeclarations":declarations}])}
    }
    Ok(result)
}

/// Keep the original model parts separately so signatures stay attached to their original parts.
#[derive(Clone,Debug)]
pub struct GoogleReply { pub chat:Value, pub model_content:Value }
pub fn response(value:&Value,model:&str)->Result<GoogleReply,String>{
    let candidates=value.get("candidates").and_then(Value::as_array).filter(|items|items.len()==1).ok_or("Réponse Google sans résultat unique.")?;
    let candidate=&candidates[0];
    let content=candidate.get("content").and_then(Value::as_object).ok_or("Contenu Google manquant.")?;
    if content.get("role").and_then(Value::as_str)!=Some("model"){return Err("Rôle Google invalide.".into())}
    let parts=content.get("parts").and_then(Value::as_array).filter(|parts|!parts.is_empty()&&parts.len()<=256).ok_or("Contenu Google invalide.")?;
    let mut text=String::new();let mut calls=Vec::new();let mut ids=std::collections::BTreeSet::new();
    for part in parts {
        let obj=part.as_object().ok_or("Fragment Google invalide.")?;
        if obj.keys().any(|key|!matches!(key.as_str(),"text"|"functionCall"|"thoughtSignature"|"thought")){return Err("Fragment Google non pris en charge.".into())}
        if let Some(signature)=obj.get("thoughtSignature"){if signature.as_str().filter(|s|!s.is_empty()&&s.len()<=65536).is_none(){return Err("Signature Google invalide.".into())}}
        if obj.get("thought").is_some_and(|v|!v.is_boolean()){return Err("Bloc de raisonnement invalide.".into())}
        if obj.get("thought").and_then(Value::as_bool)==Some(true){if !obj.get("text").is_some_and(Value::is_string)||obj.contains_key("functionCall"){return Err("Bloc de raisonnement invalide.".into())}continue}
        match (obj.get("text"),obj.get("functionCall")){
            (Some(part),None)=>text.push_str(part.as_str().ok_or("Texte Google invalide.")?),
            (None,Some(call))=>{
                if call.get("id").is_some_and(|v|v.as_str().is_none_or(|s|s.is_empty()||s.len()>256||s.chars().any(char::is_control))){return Err("Identifiant de fonction Google invalide.".into())}
                let id=call.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(||format!("nivra_call_{}",uuid::Uuid::new_v4().simple()));
                if !ids.insert(id.clone()){return Err("Identifiant de fonction Google répété.".into())}
                let name=call.get("name").and_then(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=128&&s.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-'))).ok_or("Nom de fonction Google invalide.")?;
                let args=match call.get("args"){None=>json!({}),Some(value) if value.is_object()=>value.clone(),_=>return Err("Arguments Google invalides.".into())};
                calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}));
            }
            _=>return Err("Fragment Google invalide.".into()),
        }
    }
    let finish=match candidate.get("finishReason").and_then(Value::as_str){Some("STOP") if calls.is_empty()=>"stop",Some("STOP")=>"tool_calls",Some("MAX_TOKENS") if calls.is_empty()=>"length",_=>return Err("Fin de réponse Google non prise en charge.".into())};
    let mut message=json!({"role":"assistant","content":if text.is_empty(){Value::Null}else{json!(text)}});
    if !calls.is_empty(){message["tool_calls"]=json!(calls)}
    Ok(GoogleReply{chat:json!({"model":model,"choices":[{"index":0,"message":message,"finish_reason":finish}]}),model_content:Value::Object(content.clone())})
}

/// Rebuild a tool turn from the exact model content, never reconstructing signed parts.
pub fn tool_results(reply:&GoogleReply,results:&[(String,String)])->Result<Vec<Value>,String>{
    let parts=reply.model_content.get("parts").and_then(Value::as_array).ok_or("Historique Google invalide.")?;
    let calls=reply.chat.pointer("/choices/0/message/tool_calls").and_then(Value::as_array).ok_or("Appels de fonctions absents.")?;
    let original:Vec<_>=parts.iter().filter_map(|part|part.get("functionCall")).collect();
    if calls.is_empty()||calls.len()!=original.len()||results.len()!=calls.len(){return Err("Résultats de fonctions incomplets.".into())}
    let mut expected=std::collections::BTreeMap::new();
    for (call,original) in calls.iter().zip(original){
        let bridge_id=call.get("id").and_then(Value::as_str).filter(|id|!id.is_empty()).ok_or("Identifiant de fonction manquant.")?;
        let name=original.get("name").and_then(Value::as_str).filter(|s|!s.is_empty()).ok_or("Nom de fonction manquant.")?;
        if call.pointer("/function/name").and_then(Value::as_str)!=Some(name){return Err("Fonction Google incohérente.".into())}
        let original_id=original.get("id").and_then(Value::as_str).map(str::to_owned);
        if expected.insert(bridge_id.to_owned(),(name.to_owned(),original_id)).is_some(){return Err("Identifiant de fonction répété.".into())}
    }
    let mut responses=Vec::new();
    for (id,output) in results {
        let (name,original_id)=expected.remove(id).ok_or("Résultat de fonction inconnu ou répété.")?;
        let mut response=json!({"name":name,"response":{"output":output}});
        if let Some(original_id)=original_id{response["id"]=json!(original_id)}
        responses.push(json!({"functionResponse":response}));
    }
    if !expected.is_empty(){return Err("Résultats de fonctions incomplets.".into())}
    Ok(vec![reply.model_content.clone(),json!({"role":"user","parts":responses})])
}

#[cfg(test)] mod tests{
    use super::*;
    #[test] fn converts_text_and_function_declarations(){
        let input=json!({"model":"m","messages":[{"role":"system","content":"Guide"},{"role":"user","content":"Bonjour"}],"max_tokens":80,"tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}}]});
        let result=request(&input).unwrap();assert_eq!(result["systemInstruction"]["parts"][0]["text"],"Guide");assert_eq!(result["contents"][0]["role"],"user");assert_eq!(result["generationConfig"]["maxOutputTokens"],80);assert_eq!(result["tools"][0]["functionDeclarations"][0]["name"],"read_file");
    }
    #[test] fn refuses_history_that_would_lose_tool_signatures(){
        assert!(request(&json!({"model":"m","messages":[{"role":"assistant","content":null,"tool_calls":[{"id":"x"}]}]})).is_err());
        assert!(request(&json!({"model":"m","messages":[{"role":"user","content":"hi"}],"response_format":{"type":"json_object"}})).is_err());
    }
    #[test] fn retains_signed_parts_and_function_identity(){
        let content=json!({"role":"model","parts":[{"text":"Je regarde."},{"functionCall":{"id":"call-1","name":"read_file","args":{"path":"a"}},"thoughtSignature":"c2lnbmF0dXJl"}]});
        let reply=response(&json!({"candidates":[{"content":content,"finishReason":"STOP"}]}),"google/test").unwrap();
        assert_eq!(reply.model_content,content);assert_eq!(reply.chat["choices"][0]["message"]["tool_calls"][0]["id"],"call-1");assert_eq!(reply.chat["choices"][0]["finish_reason"],"tool_calls");
    }
    #[test] fn refuses_unsupported_or_unidentified_calls(){
        assert!(response(&json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"id":"","name":"read","args":{}}}]},"finishReason":"STOP"}]}),"m").is_err());
        assert!(response(&json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"bad name","args":{}}}]},"finishReason":"STOP"}]}),"m").is_err());
    }
    #[test] fn maps_calls_without_google_ids_without_changing_signed_parts(){
        let content=json!({"role":"model","parts":[{"text":"raisonnement","thought":true,"thoughtSignature":"sig-thought"},{"text":"Je regarde.","thoughtSignature":"sig-text"},{"functionCall":{"name":"read"},"thoughtSignature":"sig-call"}]});
        let reply=response(&json!({"candidates":[{"content":content,"finishReason":"STOP"}]}),"m").unwrap();
        let id=reply.chat["choices"][0]["message"]["tool_calls"][0]["id"].as_str().unwrap().to_owned();assert!(id.starts_with("nivra_call_"));assert_eq!(reply.chat["choices"][0]["message"]["content"],"Je regarde.");assert_eq!(reply.chat["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],"{}");
        let turns=tool_results(&reply,&[(id,"ok".into())]).unwrap();assert_eq!(turns[0],content);assert_eq!(turns[1]["parts"][0]["functionResponse"]["name"],"read");assert!(turns[1]["parts"][0]["functionResponse"].get("id").is_none());
    }
    #[test] fn returns_exact_signed_parts_with_matching_results(){
        let content=json!({"role":"model","parts":[{"functionCall":{"id":"a","name":"read","args":{}},"thoughtSignature":"c2ln"},{"functionCall":{"id":"b","name":"list","args":{}}}]});
        let reply=response(&json!({"candidates":[{"content":content,"finishReason":"STOP"}]}),"m").unwrap();
        let turns=tool_results(&reply,&[("b".into(),"two".into()),("a".into(),"one".into())]).unwrap();
        assert_eq!(turns[0],content);assert_eq!(turns[1]["parts"][0]["functionResponse"]["name"],"list");assert_eq!(turns[1]["parts"][1]["functionResponse"]["id"],"a");
        assert!(tool_results(&reply,&[("a".into(),"one".into())]).is_err());
        assert!(tool_results(&reply,&[("a".into(),"one".into()),("a".into(),"again".into())]).is_err());
        assert!(tool_results(&reply,&[("a".into(),"one".into()),("unknown".into(),"two".into())]).is_err());
    }
}
