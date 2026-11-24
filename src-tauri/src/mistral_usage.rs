//! Mistral's browser approval flow and read-only Vibe Code billing requests.
use serde_json::Value;

#[cfg(windows)]
mod windows {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use reqwest::{
        blocking::{Client, RequestBuilder, Response},
        StatusCode, Url,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{
        io::Read,
        ptr, slice,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_NOT_FOUND},
        Security::Credentials::{
            CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
            CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
        },
    };
    use zeroize::{Zeroize, Zeroizing};

    const SIGN_IN: &str = "https://console.mistral.ai/api/vibe/sign-in";
    const BILLING: &str = "https://api.mistral.ai/v1/billing";
    const EXPIRED: &str = "L’autorisation Mistral a expiré. Reconnecte ton compte.";
    const CANCELLED: &str = "Connexion Mistral annulée.";
    const POLL_INTERVAL: Duration = Duration::from_secs(3);

    fn credential_target(profile: &str) -> Result<Vec<u16>, String> {
        if profile.len() != 36 || !profile.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
            return Err("Identifiant de compte invalide.".into());
        }
        Ok(format!("Nivra/Mistral/{profile}")
            .encode_utf16()
            .chain(Some(0))
            .collect())
    }

    fn save_key(mut target: Vec<u16>, key: &str) -> Result<(), String> {
        let mut secret = Zeroizing::new(key.as_bytes().to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: secret.len() as u32,
            CredentialBlob: secret.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // The two buffers remain valid until Windows has copied them.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err("Windows n’a pas pu enregistrer l’autorisation Mistral.".into());
        }
        Ok(())
    }

    fn read_key(target: &[u16]) -> Result<Zeroizing<String>, String> {
        let mut raw = ptr::null_mut();
        // CredReadW gives ownership of this allocation to the caller.
        let key = unsafe {
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) == 0 {
                return Err(if GetLastError() == ERROR_NOT_FOUND {
                    "Connecte ton compte Mistral dans ton navigateur."
                } else {
                    "Impossible de lire l’autorisation dans le Gestionnaire d’identifiants Windows."
                }
                .into());
            }
            let credential = &*raw;
            let key = if credential.CredentialBlobSize > 0 && !credential.CredentialBlob.is_null() {
                let bytes = slice::from_raw_parts_mut(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                );
                let key = std::str::from_utf8(bytes)
                    .ok()
                    .map(|value| Zeroizing::new(value.to_owned()));
                bytes.zeroize();
                key
            } else {
                None
            };
            CredFree(raw.cast());
            key
        };
        key.ok_or_else(|| "Autorisation Mistral illisible. Reconnecte ton compte.".into())
    }

    pub fn disconnect(profile: &str) -> Result<(), String> {
        let target = credential_target(profile)?;
        if unsafe {
            CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) == 0
                && GetLastError() != ERROR_NOT_FOUND
        } {
            return Err("Windows n’a pas pu supprimer l’autorisation Mistral.".into());
        }
        Ok(())
    }

    fn client() -> Result<Client, String> {
        Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Impossible de contacter Mistral.".into())
    }

    fn decode(response: Response) -> Result<Value, String> {
        const MAX_RESPONSE: u64 = 64 * 1024;
        let mut bytes = Zeroizing::new(Vec::new());
        response
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Lecture de la réponse Mistral interrompue.")?;
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err("Réponse Mistral trop volumineuse.".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "Réponse Mistral illisible.".into())
    }

    fn json_body(request: RequestBuilder, body: Value) -> RequestBuilder {
        request
            .header("Content-Type", "application/json")
            .body(body.to_string())
    }

    fn returned_url(value: &Value, path_prefix: &str) -> Result<Url, String> {
        value
            .as_str()
            .and_then(|value| Url::parse(value).ok())
            .filter(|url| {
                url.scheme() == "https"
                    && url.host_str() == Some("console.mistral.ai")
                    && url.port_or_known_default() == Some(443)
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none()
                    && url.path().starts_with(path_prefix)
            })
            .ok_or_else(|| "Mistral a fourni une adresse de connexion inattendue.".into())
    }

    fn challenge(verifier: &str) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }

    fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
        if cancelled.load(Ordering::Relaxed) {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }

    fn await_approval(
        client: &Client,
        poll: Url,
        cancelled: &AtomicBool,
    ) -> Result<Zeroizing<String>, String> {
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut failures = 0;
        while Instant::now() < deadline {
            check_cancelled(cancelled)?;
            let mut interval = POLL_INTERVAL;
            match client.get(poll.clone()).send() {
                Ok(response) => match response.status() {
                    StatusCode::OK => {
                        let mut payload = decode(response)?;
                        failures = 0;
                        match payload["status"].as_str() {
                            Some("pending") => {}
                            Some("completed") => {
                                return take_secret(&mut payload, "exchange_token", 8192)
                            }
                            Some("expired") => return Err(EXPIRED.into()),
                            Some("denied") => {
                                return Err("La connexion a été refusée dans Mistral.".into())
                            }
                            _ => return Err("Mistral n’a pas pu autoriser la connexion.".into()),
                        }
                    }
                    StatusCode::GONE => return Err(EXPIRED.into()),
                    StatusCode::TOO_MANY_REQUESTS => {
                        let seconds = response
                            .headers()
                            .get("retry-after")
                            .and_then(|header| header.to_str().ok())
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(15);
                        interval = Duration::from_secs(seconds.max(3));
                    }
                    status if status.is_server_error() => failures += 1,
                    _ => {
                        return Err(
                            "Impossible de vérifier l’autorisation auprès de Mistral.".into()
                        )
                    }
                },
                Err(_) => failures += 1,
            }
            if failures >= 3 {
                return Err("Mistral ne répond plus. Réessaie dans un instant.".into());
            }
            let next =
                Instant::now() + interval.min(deadline.saturating_duration_since(Instant::now()));
            while Instant::now() < next {
                check_cancelled(cancelled)?;
                std::thread::sleep(
                    Duration::from_millis(200).min(next.saturating_duration_since(Instant::now())),
                );
            }
        }
        Err(EXPIRED.into())
    }

    fn take_secret(
        payload: &mut Value,
        field: &str,
        max_len: usize,
    ) -> Result<Zeroizing<String>, String> {
        if let Value::String(value) = payload[field].take() {
            let secret = Zeroizing::new(value);
            if !secret.is_empty()
                && secret.len() <= max_len
                && secret.bytes().all(|byte| byte.is_ascii_graphic())
            {
                return Ok(secret);
            }
        }
        Err("Mistral n’a pas terminé l’autorisation.".into())
    }

    fn authorize(client: &Client, cancelled: &AtomicBool) -> Result<Zeroizing<String>, String> {
        let mut random = Zeroizing::new([0_u8; 64]);
        getrandom::fill(&mut *random)
            .map_err(|_| "Impossible de préparer la connexion Mistral.")?;
        let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(&*random));
        check_cancelled(cancelled)?;
        let response = json_body(
            client.post(SIGN_IN),
            json!({
                "code_challenge": challenge(&verifier),
                "code_challenge_method": "S256",
            }),
        )
        .send()
        .map_err(|_| "Impossible de joindre la page de connexion Mistral.")?;
        if !response.status().is_success() {
            return Err(format!(
                "Mistral n’a pas pu lancer la connexion (HTTP {}).",
                response.status().as_u16()
            ));
        }
        let start = decode(response)?;
        let process = start["process_id"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
            .ok_or("Mistral n’a pas fourni de session de connexion.")?;
        let page = returned_url(&start["sign_in_url"], "/")?;
        let poll = returned_url(&start["poll_url"], "/api/vibe/sign-in/")?;
        check_cancelled(cancelled)?;
        // Uses the Windows association for HTTPS, the same helper as other service links.
        crate::browser::open(page.as_str())?;

        let token = await_approval(client, poll, cancelled)?;
        check_cancelled(cancelled)?;
        let response = json_body(
            client.post(format!("{SIGN_IN}/{process}/exchange")),
            json!({
                "exchange_token": token.as_str(),
                "code_verifier": verifier.as_str(),
            }),
        )
        .send()
        .map_err(|_| "Mistral n’a pas pu terminer l’autorisation.")?;
        if response.status() == StatusCode::FORBIDDEN {
            return Err("Cette organisation n’autorise pas la connexion. Réessaie avec l’organisation de ton abonnement Vibe Code.".into());
        }
        if !response.status().is_success() {
            return Err("Mistral n’a pas pu terminer l’autorisation.".into());
        }
        take_secret(
            &mut decode(response)?,
            "api_key",
            CRED_MAX_CREDENTIAL_BLOB_SIZE as usize,
        )
    }

    fn billing(client: &Client, key: &str, resource: &str) -> Result<Value, String> {
        let response = client
            .get(format!("{BILLING}/{resource}"))
            .bearer_auth(key)
            .send()
            .map_err(|_| "Impossible de joindre Mistral pour lire le quota.")?;
        match response.status() {
            StatusCode::UNAUTHORIZED => Err(EXPIRED.into()),
            StatusCode::FORBIDDEN => Err(
                "Ton compte ne peut pas consulter le quota de cette organisation Mistral.".into(),
            ),
            status if !status.is_success() => Err(format!(
                "Lecture du quota Mistral impossible (HTTP {}).",
                status.as_u16()
            )),
            _ => decode(response),
        }
    }

    fn parse_budget(response: &Value) -> Result<Value, String> {
        let budget = &response["vibe_budget"];
        let number = |field: &str| {
            budget[field]
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0)
        };
        let used = number("usage_percentage")
            .ok_or("Mistral n’a pas communiqué le quota mensuel Vibe Code.")?;
        let limit =
            number("initial_budget").ok_or("Mistral n’a pas communiqué le plafond Vibe Code.")?;
        let currency = budget["currency"]
            .as_str()
            .filter(|code| code.len() == 3 && code.bytes().all(|c| c.is_ascii_alphabetic()))
            .ok_or("Mistral n’a pas communiqué la devise du quota Vibe Code.")?;
        Ok(json!({
            "usedPercent": used, "limit": limit, "currency": currency,
            "resetAt": budget["reset_at"].as_str(),
        }))
    }

    fn fetch(client: &Client, key: &str) -> Result<Value, String> {
        let mut quota = parse_budget(&billing(client, key, "budget")?)?;
        quota["plan"] = billing(client, key, "subscription")
            .ok()
            .map(|value| value["plan_name"].clone())
            .filter(Value::is_string)
            .unwrap_or(Value::Null);
        Ok(quota)
    }

    pub fn login(profile: &str, cancelled: &AtomicBool) -> Result<Value, String> {
        let target = credential_target(profile)?;
        let client = client()?;
        let key = authorize(&client, cancelled)?;
        check_cancelled(cancelled)?;
        let quota = fetch(&client, &key)?;
        check_cancelled(cancelled)?;
        save_key(target, &key)?;
        Ok(quota)
    }

    pub fn usage(profile: &str) -> Result<Value, String> {
        let key = read_key(&credential_target(profile)?)?;
        fetch(&client()?, &key)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn makes_the_standard_pkce_challenge() {
            assert_eq!(
                challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
            );
        }
        #[test]
        fn opens_only_the_official_browser_approval_origin() {
            assert!(returned_url(
                &json!("https://console.mistral.ai/codestral/cli/authenticate?process_id=test"),
                "/"
            )
            .is_ok());
            for url in [
                "https://console.mistral.ai.example.com/api/vibe/sign-in/poll/test",
                "https://console.mistral.ai:8443/api/vibe/sign-in/poll/test",
                "https://console.mistral.ai/account",
                "http://console.mistral.ai/api/vibe/sign-in/poll/test",
            ] {
                assert!(returned_url(&json!(url), "/api/vibe/sign-in/").is_err());
            }
        }
        #[test]
        fn reads_the_vibe_code_allowance_with_currency_and_reset() {
            let data = json!({
                "api_budget": {"usage_percentage": 100, "initial_budget": 15, "currency": "usd"},
                "vibe_budget": {"usage_percentage": 0, "initial_budget": 255, "currency": "eur", "reset_at": "2026-12-01"},
            });
            assert_eq!(
                parse_budget(&data).unwrap(),
                json!({
                    "usedPercent": 0.0, "limit": 255.0, "currency": "eur", "resetAt": "2026-12-01",
                })
            );
            assert!(parse_budget(&json!({"api_budget": data["api_budget"]})).is_err());
            assert!(parse_budget(&json!({"vibe_budget": {"usage_percentage": 0}})).is_err());
        }
    }
}

#[tauri::command]
pub async fn login_mistral(profile: String, attempt: String) -> Result<Value, String> {
    #[cfg(windows)]
    {
        let (cancelled, guard) = crate::register_login(&attempt)?;
        tauri::async_runtime::spawn_blocking(move || {
            let _guard = guard;
            windows::login(&profile, &cancelled)
        })
        .await
        .map_err(|_| "Connexion Mistral interrompue.".to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (profile, attempt);
        Err("La connexion Mistral est disponible dans Nivra pour Windows.".into())
    }
}

#[tauri::command]
pub async fn mistral_usage(profile: String) -> Result<Value, String> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move || windows::usage(&profile))
        .await
        .map_err(|_| "Lecture du quota Mistral interrompue.".to_string())?;
    #[cfg(not(windows))]
    {
        let _ = profile;
        Err("La connexion Mistral est disponible dans Nivra pour Windows.".into())
    }
}

#[tauri::command]
pub fn disconnect_mistral(profile: String) -> Result<(), String> {
    #[cfg(windows)]
    return windows::disconnect(&profile);
    #[cfg(not(windows))]
    {
        let _ = profile;
        Err("La connexion Mistral est disponible dans Nivra pour Windows.".into())
    }
}
