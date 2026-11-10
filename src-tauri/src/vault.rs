//! Windows Credential Manager storage for explicitly saved provider keys.
//! A credential is bound to the configured endpoint and protocol.
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize, Deserialize)]
struct StoredKey { base_url:String, chat_only:bool, #[serde(default)] anthropic_native:bool, key:String }
impl Drop for StoredKey { fn drop(&mut self) { self.key.zeroize(); } }

fn target(reference: &str) -> Result<String, String> {
    if reference.len()!=32 || !reference.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) { return Err("Référence de clé invalide.".into()); }
    Ok(format!("Nivra/Gateway/{reference}"))
}

fn validate_binding(record: &StoredKey, base_url: &str, chat_only: bool, anthropic_native: bool) -> Result<(), String> {
    let canonical=crate::gateway::upstream_url(base_url)?.to_string();
    if record.base_url!=canonical || record.chat_only!=chat_only || record.anthropic_native!=anthropic_native { return Err("La clé enregistrée ne correspond pas à cette adresse ou à ce protocole. Reconfigure le fournisseur.".into()); }
    Ok(())
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::{Foundation::{GetLastError, ERROR_NOT_FOUND}, Security::Credentials::{CredWriteW, CredReadW, CredDeleteW, CredFree, CREDENTIALW, CRED_TYPE_GENERIC, CRED_PERSIST_LOCAL_MACHINE}};
    use std::ptr;

    fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(std::iter::once(0)).collect() }
    struct Handle(*mut CREDENTIALW);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let credential=&mut *self.0;
                if !credential.CredentialBlob.is_null() && credential.CredentialBlobSize<=2560 {
                    std::slice::from_raw_parts_mut(credential.CredentialBlob,credential.CredentialBlobSize as usize).zeroize();
                }
                CredFree(self.0.cast());
            }
        }
    }

    pub fn write(reference: &str, data: &mut [u8]) -> Result<(), String> {
        if data.len()>2560 { return Err("Clé ou adresse trop longue pour le coffre Windows.".into()); }
        let mut name=wide(&target(reference)?);
        let mut user=wide("Nivra");
        let credential=CREDENTIALW { Type:CRED_TYPE_GENERIC, TargetName:name.as_mut_ptr(), CredentialBlobSize:data.len() as u32, CredentialBlob:data.as_mut_ptr(), Persist:CRED_PERSIST_LOCAL_MACHINE, UserName:user.as_mut_ptr(), ..Default::default() };
        if unsafe { CredWriteW(&credential,0) }==0 { return Err("Impossible d'enregistrer la clé dans le coffre Windows.".into()); }
        Ok(())
    }
    pub fn read(reference: &str) -> Result<Zeroizing<Vec<u8>>, String> {
        let name=wide(&target(reference)?);
        let mut raw: *mut CREDENTIALW=ptr::null_mut();
        if unsafe { CredReadW(name.as_ptr(),CRED_TYPE_GENERIC,0,&mut raw) }==0 { return Err("Clé introuvable dans le coffre Windows. Ajoute-la à nouveau.".into()); }
        let handle=Handle(raw);
        let entry=unsafe { &*handle.0 };
        if entry.CredentialBlobSize==0 || entry.CredentialBlobSize>2560 || entry.CredentialBlob.is_null() { return Err("Clé enregistrée invalide.".into()); }
        Ok(Zeroizing::new(unsafe { std::slice::from_raw_parts(entry.CredentialBlob,entry.CredentialBlobSize as usize) }.to_vec()))
    }
    pub fn delete(reference: &str) -> Result<(), String> {
        let name=wide(&target(reference)?);
        if unsafe { CredDeleteW(name.as_ptr(),CRED_TYPE_GENERIC,0) }==0 && unsafe { GetLastError() }!=ERROR_NOT_FOUND { return Err("Impossible de retirer la clé du coffre Windows.".into()); }
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    pub fn write(_: &str, _: &mut [u8]) -> Result<(), String> { Err("Le coffre Windows est indisponible sur cette plateforme.".into()) }
    pub fn read(_: &str) -> Result<Zeroizing<Vec<u8>>, String> { Err("Le coffre Windows est indisponible sur cette plateforme.".into()) }
    pub fn delete(_: &str) -> Result<(), String> { Err("Le coffre Windows est indisponible sur cette plateforme.".into()) }
}

#[tauri::command]
pub fn gateway_remember_key(base_url:String, api_key:String, chat_only:bool, anthropic_native:bool) -> Result<String, String> {
    let key=Zeroizing::new(api_key);
    if key.is_empty() || key.contains(['\r','\n']) { return Err("Clé fournisseur invalide.".into()); }
    let base_url=crate::gateway::upstream_url(&base_url)?.to_string();
    if chat_only && anthropic_native { return Err("Protocoles incompatibles.".into()); }
    let record=StoredKey { base_url, chat_only, anthropic_native, key:key.to_string() };
    let mut bytes=Zeroizing::new(serde_json::to_vec(&record).map_err(|_| "Impossible de préparer la clé.")?);
    let reference=uuid::Uuid::new_v4().simple().to_string();
    platform::write(&reference,&mut bytes)?;
    Ok(reference)
}

pub fn load_key(reference:&str, base_url:&str, chat_only:bool, anthropic_native:bool) -> Result<String, String> {
    let bytes=platform::read(reference)?;
    let record:StoredKey=serde_json::from_slice(&bytes).map_err(|_| "Clé enregistrée invalide.")?;
    validate_binding(&record,base_url,chat_only,anthropic_native)?;
    Ok(record.key.clone())
}

pub fn replace_key(reference:&str,base_url:&str,chat_only:bool,anthropic_native:bool,new_key:&str)->Result<(),String>{
    if new_key.is_empty()||new_key.contains(['\r','\n']){return Err("Clé fournisseur invalide.".into())}
    let bytes=platform::read(reference)?;
    let record:StoredKey=serde_json::from_slice(&bytes).map_err(|_|"Clé enregistrée invalide.")?;
    validate_binding(&record,base_url,chat_only,anthropic_native)?;
    let updated=StoredKey{base_url:record.base_url.clone(),chat_only,anthropic_native,key:new_key.to_owned()};
    let mut encoded=Zeroizing::new(serde_json::to_vec(&updated).map_err(|_|"Impossible de préparer la clé.")?);
    platform::write(reference,&mut encoded)
}

#[tauri::command]
pub fn gateway_forget_key(reference:String) -> Result<(), String> { platform::delete(&reference) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_reference_is_confined_to_nivra_namespace() {
        assert!(target("deadbeefdeadbeefdeadbeefdeadbeef").unwrap().starts_with("Nivra/Gateway/"));
        assert!(target("../../other-service").is_err());
    }
    #[test]
    fn key_is_bound_to_endpoint_and_protocol() {
        let record=StoredKey{base_url:"https://example.com/v1/".into(),chat_only:true,anthropic_native:false,key:"test".into()};
        assert!(validate_binding(&record,"https://example.com/v1",true,false).is_ok());
        assert!(validate_binding(&record,"https://evil.example/v1",true,false).is_err());
        assert!(validate_binding(&record,"https://example.com/v1",false,false).is_err());
        assert!(validate_binding(&record,"https://example.com/v1",true,true).is_err());
        let legacy:StoredKey=serde_json::from_str(r#"{"base_url":"https://example.com/v1/","chat_only":true,"key":"test"}"#).unwrap();
        assert!(validate_binding(&legacy,"https://example.com/v1",true,false).is_ok());
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "Uses a temporary synthetic Windows Credential Manager entry"]
    fn windows_vault_roundtrip_uses_endpoint_binding() {
        struct Cleanup(String);
        impl Drop for Cleanup { fn drop(&mut self) { let _=platform::delete(&self.0); } }
        let reference=gateway_remember_key("https://example.com/v1".into(),"synthetic-test-key".into(),true,false).unwrap();
        let _cleanup=Cleanup(reference.clone());
        assert_eq!(load_key(&reference,"https://example.com/v1",true,false).unwrap(),"synthetic-test-key");
        assert!(load_key(&reference,"https://example.org/v1",true,false).is_err());
        assert!(load_key(&reference,"https://example.com/v1",false,false).is_err());
        assert!(replace_key(&reference,"https://example.org/v1",true,false,"wrong").is_err());
        assert_eq!(load_key(&reference,"https://example.com/v1",true,false).unwrap(),"synthetic-test-key");
        replace_key(&reference,"https://example.com/v1",true,false,"synthetic-rotated-key").unwrap();
        assert_eq!(load_key(&reference,"https://example.com/v1",true,false).unwrap(),"synthetic-rotated-key");
    }
}
