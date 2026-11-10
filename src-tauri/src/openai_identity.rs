//! Identity verification for Nivra's future Sign in with ChatGPT flow.
//! Never trust decoded claims until the OpenAI signature and transaction are verified.
use jsonwebtoken::{decode,decode_header,Algorithm,DecodingKey,Validation,jwk::{JwkSet,AlgorithmParameters,KeyAlgorithm,PublicKeyUse,KeyOperations}};
use serde::Deserialize;

#[derive(Debug,Deserialize)]
struct Claims{iss:String,sub:String,aud:serde_json::Value,exp:u64,iat:u64,nonce:String,email:Option<String>}
#[derive(Debug,PartialEq)]
pub(crate) struct VerifiedIdentity{pub subject:String,pub email:Option<String>}

pub(crate) fn verify(id_token:&str,client_id:&str,nonce:&str,jwks:&JwkSet)->Result<VerifiedIdentity,String>{
 if id_token.len()>32768||client_id.is_empty()||client_id.len()>256||nonce.is_empty()||nonce.len()>256||jwks.keys.len()>64{return Err("Identité OpenAI invalide.".into())}
 let header=decode_header(id_token).map_err(|_|"Jeton d’identité OpenAI invalide.")?;
 if header.alg!=Algorithm::RS256{return Err("Algorithme OpenAI inattendu.".into())}
 let kid=header.kid.as_deref().filter(|v|!v.is_empty()&&v.len()<=256).ok_or("Clé d’identité OpenAI absente.")?;
 let mut matching=jwks.keys.iter().filter(|key|key.common.key_id.as_deref()==Some(kid));let key=matching.next().ok_or("Clé d’identité OpenAI introuvable.")?;
 if matching.next().is_some()||!matches!(key.algorithm,AlgorithmParameters::RSA(_))||key.common.key_algorithm.is_some_and(|alg|alg!=KeyAlgorithm::RS256)||key.common.public_key_use.as_ref().is_some_and(|usage|*usage!=PublicKeyUse::Signature)||key.common.key_operations.as_ref().is_some_and(|ops|!ops.contains(&KeyOperations::Verify)){return Err("Clé d’identité OpenAI invalide.".into())}
 let key=DecodingKey::from_jwk(key).map_err(|_|"Clé d’identité OpenAI invalide.")?;
 let mut validation=Validation::new(Algorithm::RS256);validation.set_issuer(&["https://auth.openai.com"]);validation.set_audience(&[client_id]);validation.set_required_spec_claims(&["exp","iss","aud","sub"]);validation.leeway=5;validation.validate_nbf=true;
 let claims=decode::<Claims>(id_token,&key,&validation).map_err(|_|"Vérification de l’identité OpenAI impossible.")?.claims;
 let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|"Horloge système invalide.")?.as_secs();
 if claims.nonce!=nonce||claims.sub.is_empty()||claims.sub.len()>512||claims.sub.chars().any(char::is_control)||claims.iat>now.saturating_add(5){return Err("Identité OpenAI incohérente.".into())}
 let _=(&claims.iss,&claims.aud,claims.exp);
 let email=claims.email.filter(|v|v.len()<=320&&!v.chars().any(char::is_control));
 Ok(VerifiedIdentity{subject:claims.sub,email})
}

pub(crate) fn plan_scope(scope:&str)->bool{scope.split_ascii_whitespace().any(|v|v=="chatgpt.tokens.use.direct")}

#[cfg(test)] mod tests{
 use super::*;use serde_json::json;use jsonwebtoken::{encode,EncodingKey,Header};
 const MODULUS:&str="yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";
 fn keys()->JwkSet{serde_json::from_value(json!({"keys":[{"kty":"RSA","n":MODULUS,"e":"AQAB","kid":"test","alg":"RS256","use":"sig"}]})).unwrap()}
 fn sign(claims:serde_json::Value)->String{let mut header=Header::new(Algorithm::RS256);header.kid=Some("test".into());encode(&header,&claims,&EncodingKey::from_rsa_pem(include_bytes!("../tests/fixtures/synthetic-rsa.pem")).unwrap()).unwrap()}
 fn claims()->serde_json::Value{let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();json!({"iss":"https://auth.openai.com","sub":"synthetic-user","aud":"oaiapp_test","exp":now+3600,"iat":now,"nonce":"expected","email":"test@example.invalid"})}
 #[test] fn verifies_signature_and_transaction_claims(){let token=sign(claims());assert_eq!(verify(&token,"oaiapp_test","expected",&keys()).unwrap().subject,"synthetic-user");assert!(verify(&token,"oaiapp_other","expected",&keys()).is_err());assert!(verify(&token,"oaiapp_test","wrong",&keys()).is_err());let mut altered=claims();altered["iss"]=json!("https://evil.example");assert!(verify(&sign(altered),"oaiapp_test","expected",&keys()).is_err());let mut expired=claims();expired["exp"]=json!(1);assert!(verify(&sign(expired),"oaiapp_test","expected",&keys()).is_err());let mut parts:Vec<_>=token.split('.').map(str::to_owned).collect();let replacement=if parts[2].starts_with('A'){"B"}else{"A"};parts[2].replace_range(0..1,replacement);assert!(verify(&parts.join("."),"oaiapp_test","expected",&keys()).is_err());}
 #[test] fn requires_granted_plan_scope(){assert!(plan_scope("openid chatgpt.tokens.use.direct offline_access"));assert!(!plan_scope("openid chatgpt.tokens.use.direct.extra"));}
}
