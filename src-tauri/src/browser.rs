//! Open web links through the Windows URL association, without Explorer's command-line parser.
fn web_url(raw: &str) -> Result<String, String> {
    if raw.len() > 16384 || raw.chars().any(char::is_control) {
        return Err("Adresse web invalide.".into());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "Adresse web invalide.")?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err("Adresse web HTTPS requise.".into());
    }
    Ok(url.into())
}

pub fn open(raw: &str) -> Result<(), String> {
    let url = web_url(raw)?;
    #[cfg(windows)] {
        std::thread::spawn(move || {
            use windows_sys::Win32::{System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE}, UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL}};
            let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
            let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
            // Both UTF-16 strings live through the call; no arguments are interpreted by a shell.
            let result = unsafe {
                let initialized = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
                let result = ShellExecuteW(std::ptr::null_mut(), operation.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) as isize;
                if initialized >= 0 { CoUninitialize(); }
                result
            };
            if result > 32 { Ok(()) } else { Err("Impossible d’ouvrir le navigateur par défaut. Vérifie l’application associée aux liens HTTPS dans Windows.".into()) }
        }).join().map_err(|_| "Ouverture du navigateur interrompue.".to_string())?
    }
    #[cfg(not(windows))] { let _ = url; Err("Ouverture du navigateur disponible sur Windows.".into()) }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn keeps_oauth_query_intact() {
        let url = "https://accounts.google.com/o/oauth2/v2/auth?redirect_uri=http%3A%2F%2F127.0.0.1%3A51121%2Fcallback&scope=one+two&state=a-b&code_challenge=x_y";
        assert_eq!(web_url(url).unwrap(), url);
    }
    #[test] fn rejects_files_commands_and_embedded_credentials() {
        for url in ["C:\\Users\\dylan", "file:///C:/Users", "cmd.exe", "javascript:alert(1)", "https://user:password@example.com", "https://example.com\0hidden"] {
            assert!(web_url(url).is_err());
        }
    }
}
