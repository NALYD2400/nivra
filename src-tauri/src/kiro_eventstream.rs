//! Incremental AWS EventStream decoding for the Kiro runtime.
use std::collections::BTreeMap;

const MAX_FRAME:usize=2*1024*1024;
const MAX_BUFFER:usize=16*1024*1024;
#[derive(Debug,PartialEq)]pub struct Event{pub headers:BTreeMap<String,String>,pub payload:Vec<u8>}
#[derive(Default)]pub struct Decoder{buffer:Vec<u8>}

fn number(bytes:&[u8])->u32{u32::from_be_bytes(bytes.try_into().expect("four bytes"))}
fn headers(data:&[u8])->Result<BTreeMap<String,String>,String>{
 let mut pos=0;let mut headers=BTreeMap::new();
 while pos<data.len(){
  let length=data[pos] as usize;pos+=1;if length==0||pos+length+1>data.len(){return Err("En-tête Kiro invalide.".into())}
  let name=std::str::from_utf8(&data[pos..pos+length]).map_err(|_|"En-tête Kiro illisible.")?.to_owned();pos+=length;let kind=data[pos];pos+=1;
  let (len,prefix)=match kind{0|1=>(0,0),2=>(1,0),3=>(2,0),4=>(4,0),5|8=>(8,0),9=>(16,0),6|7=>{if pos+2>data.len(){return Err("En-tête Kiro tronqué.".into())}(u16::from_be_bytes([data[pos],data[pos+1]]) as usize,2)},_=>return Err("Type d’en-tête Kiro inconnu.".into())};
  pos+=prefix;if pos+len>data.len(){return Err("En-tête Kiro tronqué.".into())}
  let value=if kind==7{std::str::from_utf8(&data[pos..pos+len]).map_err(|_|"En-tête Kiro illisible.")?.to_owned()}else{String::new()};pos+=len;
  if headers.insert(name,value).is_some(){return Err("En-tête Kiro dupliqué.".into())}
 }Ok(headers)
}
impl Decoder{
 pub fn ingest(&mut self,chunk:&[u8])->Result<Vec<Event>,String>{
  if self.buffer.len().saturating_add(chunk.len())>MAX_BUFFER{return Err("Flux Kiro trop volumineux.".into())}self.buffer.extend_from_slice(chunk);let mut result=Vec::new();let mut consumed=0;
  while self.buffer.len()-consumed>=12{
   let rest=&self.buffer[consumed..];let total=number(&rest[..4]) as usize;let header_len=number(&rest[4..8]) as usize;
   if !(16..=MAX_FRAME).contains(&total)||header_len>total-16{return Err("Taille de message Kiro invalide.".into())}
   if crc32fast::hash(&rest[..8])!=number(&rest[8..12]){return Err("En-tête de flux Kiro corrompu.".into())}
   if rest.len()<total{break}
   if crc32fast::hash(&rest[..total-4])!=number(&rest[total-4..total]){return Err("Message Kiro corrompu.".into())}
   let parsed=headers(&rest[12..12+header_len])?;let payload=rest[12+header_len..total-4].to_vec();result.push(Event{headers:parsed,payload});consumed+=total;
  }
  if consumed>0{self.buffer.drain(..consumed);}Ok(result)
 }
 pub fn finish(&self)->Result<(),String>{if self.buffer.is_empty(){Ok(())}else{Err("Flux Kiro terminé prématurément.".into())}}
}
#[cfg(test)]pub(crate) mod tests{
 use super::*;
 pub(crate) fn frame(name:&str,payload:&[u8])->Vec<u8>{frame_headers(&[(":message-type","event"),(":event-type",name)],payload)}
 pub(crate) fn frame_headers(headers:&[(&str,&str)],payload:&[u8])->Vec<u8>{let mut h=Vec::new();for(k,v)in headers{h.push(k.len() as u8);h.extend_from_slice(k.as_bytes());h.push(7);h.extend_from_slice(&(v.len() as u16).to_be_bytes());h.extend_from_slice(v.as_bytes())}let mut out=Vec::new();out.extend_from_slice(&((16+h.len()+payload.len()) as u32).to_be_bytes());out.extend_from_slice(&(h.len() as u32).to_be_bytes());out.extend_from_slice(&crc32fast::hash(&out).to_be_bytes());out.extend_from_slice(&h);out.extend_from_slice(payload);out.extend_from_slice(&crc32fast::hash(&out).to_be_bytes());out}
 #[test]fn handles_fragmented_and_coalesced_events(){let first=frame("assistantResponseEvent",br#"{"content":"Bonjour"}"#);let second=frame("messageStopEvent",b"{}");let mut wire=first.clone();wire.extend_from_slice(&second);let mut decoder=Decoder::default();let mut result=Vec::new();for chunk in wire.chunks(3){result.extend(decoder.ingest(chunk).unwrap())}decoder.finish().unwrap();assert_eq!(result.len(),2);assert_eq!(result[0].headers[":event-type"],"assistantResponseEvent");assert_eq!(result[0].payload,br#"{"content":"Bonjour"}"#);assert_eq!(result[1].headers[":event-type"],"messageStopEvent");}
 #[test]fn detects_corruption_and_truncation(){let good=frame("test",b"{}");let mut broken=good.clone();broken[8]^=1;assert!(Decoder::default().ingest(&broken).is_err());let mut broken=good.clone();let i=broken.len()-5;broken[i]^=1;assert!(Decoder::default().ingest(&broken).is_err());let mut incomplete=Decoder::default();incomplete.ingest(&good[..good.len()-1]).unwrap();assert!(incomplete.finish().is_err());}
 #[test]fn rejects_bad_lengths_and_duplicate_headers(){let mut frame=Vec::new();frame.extend_from_slice(&15u32.to_be_bytes());frame.extend_from_slice(&0u32.to_be_bytes());frame.extend_from_slice(&crc32fast::hash(&frame).to_be_bytes());assert!(Decoder::default().ingest(&frame).is_err());let duplicate=[1,b'x',7,0,1,b'a',1,b'x',7,0,1,b'b'];assert!(headers(&duplicate).is_err());}
}
