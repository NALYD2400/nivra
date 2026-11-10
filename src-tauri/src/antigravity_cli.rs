//! Read the official Antigravity CLI's model list on explicit user request.
use serde::Serialize;
use std::{io::Read, process::{Command,Stdio}, sync::atomic::{AtomicBool,Ordering}, time::{Duration,Instant}};
#[cfg(windows)] use std::os::windows::process::CommandExt;

#[derive(Serialize,Debug,PartialEq)]
pub struct CliModel { id:String, label:String }
static READING:AtomicBool=AtomicBool::new(false);
struct ReadingGuard;
impl Drop for ReadingGuard{fn drop(&mut self){READING.store(false,Ordering::Release)}}

#[cfg(windows)]
fn executable()->std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).map(|path|path.join("agy/bin/agy.exe")).filter(|path|path.is_file()).unwrap_or_else(||"agy.exe".into())
}

fn valid_model(model:&str)->bool {
    !model.is_empty() && model.len()<=200 && !model.starts_with('-') && !model.chars().any(|c|c.is_whitespace()||c.is_control())
}

#[cfg(windows)]
fn launch_command(executable:&std::path::Path,folder:&std::path::Path,model:&str)->Command {
    let mut command=Command::new(executable);
    command.arg("--model").arg(model).current_dir(folder).creation_flags(0x00000010);
    command
}

#[cfg(windows)]
fn login_command(executable:&std::path::Path)->Command {
    let mut command=Command::new(executable);
    command.args(["acp","--login"]).creation_flags(0x00000010);
    command
}

#[tauri::command]
pub fn antigravity_cli_login()->Result<(),String>{
    #[cfg(windows)] {
        login_command(&executable()).spawn().map_err(|_|"Impossible d’ouvrir la connexion Antigravity CLI. Vérifie son installation.".to_string())?;
        Ok(())
    }
    #[cfg(not(windows))] {Err("La connexion Antigravity CLI est disponible uniquement sur Windows.".into())}
}

#[tauri::command]
pub fn antigravity_cli_launch(app:tauri::AppHandle,model:String,workspace:String)->Result<(),String>{
    if !valid_model(&model){return Err("Identifiant de modèle invalide.".into())}
    #[cfg(windows)] {
        use tauri::Manager;
        let folder=if workspace.trim().is_empty(){app.path().document_dir().map_err(|_|"Dossier Documents inaccessible.")?}else{std::path::PathBuf::from(workspace.trim())};
        let folder=folder.canonicalize().map_err(|_|"Dossier du projet introuvable.")?;
        if !folder.is_dir(){return Err("Choisis un dossier de projet valide.".into())}
        launch_command(&executable(),&folder,&model).spawn().map_err(|_|"Impossible d’ouvrir Antigravity CLI. Vérifie son installation.".to_string())?;
        Ok(())
    }
    #[cfg(not(windows))] {let _=(app,workspace);Err("L’ouverture d’Antigravity CLI est disponible uniquement sur Windows.".into())}
}

fn parse_models(output:&str)->Vec<CliModel>{
    let mut models=Vec::new();let mut seen=std::collections::BTreeSet::new();
    for line in output.lines(){
        let Some((id,label))=line.split_once('\t') else{continue};
        let id=id.trim();let label=label.trim();
        if id.is_empty()||id.len()>200||label.is_empty()||label.len()>300||id.chars().any(char::is_control)||label.chars().any(char::is_control)||!seen.insert(id.to_string()){continue}
        models.push(CliModel{id:id.to_string(),label:label.to_string()});
        if models.len()>=200{break}
    }
    models
}

#[tauri::command]
pub async fn antigravity_cli_models()->Result<Vec<CliModel>,String>{
    tauri::async_runtime::spawn_blocking(move||{
        if READING.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("Une lecture des modèles Antigravity est déjà en cours.".into())}
        let guard=std::sync::Arc::new(ReadingGuard);
        #[cfg(windows)]
        let executable=executable();
        #[cfg(not(windows))]
        let executable=std::path::PathBuf::from("agy");
        let mut command=Command::new(executable);
        command.arg("models").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)] command.creation_flags(0x08000000);
        let mut child=command.spawn().map_err(|_|"Antigravity CLI est introuvable. Installe-le puis réessaie.".to_string())?;
        let stdout=match child.stdout.take(){Some(stdout)=>stdout,None=>{let _=child.kill();let _=child.wait();return Err("Impossible de lire Antigravity CLI.".into())}};
        let (sender,receiver)=std::sync::mpsc::sync_channel(1);
        let reader_guard=guard.clone();
        std::thread::spawn(move||{let _guard=reader_guard;let mut bytes=Vec::new();let result=stdout.take(1_048_577).read_to_end(&mut bytes);let _=sender.send((result,bytes));});
        let deadline=Instant::now()+Duration::from_secs(20);
        let status=loop{
            match child.try_wait(){
                Ok(Some(status))=>break status,
                Ok(None) if Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(100)),
                _=>{let _=child.kill();let _=child.wait();return Err("La lecture des modèles Antigravity a expiré ou a été interrompue.".into())}
            }
        };
        let (result,bytes)=receiver.recv_timeout(Duration::from_secs(2)).map_err(|_|"Lecture Antigravity interrompue.")?;
        if result.is_err()||bytes.len()>1_048_576{return Err("Réponse Antigravity illisible ou trop volumineuse.".into())}
        if !status.success(){return Err("Antigravity CLI n'a pas fourni de modèles. Vérifie sa connexion puis réessaie.".into())}
        let output=String::from_utf8(bytes).map_err(|_|"Réponse Antigravity illisible.")?;
        let models=parse_models(&output);
        if models.is_empty(){return Err("Aucun modèle lisible n'a été renvoyé par Antigravity CLI.".into())}
        Ok(models)
    }).await.map_err(|_|"Lecture Antigravity interrompue.".to_string())?
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn parses_cli_output_without_treating_status_as_a_model(){
        let models=parse_models("Fetching available models...\ngemini-high\tGemini (High)\nclaude-opus\tClaude Opus\ngemini-high\tDuplicate\ninvalid\n");
        assert_eq!(models,vec![CliModel{id:"gemini-high".into(),label:"Gemini (High)".into()},CliModel{id:"claude-opus".into(),label:"Claude Opus".into()}]);
    }
    #[test]
    fn discards_invalid_or_duplicate_lines(){
        let models=parse_models("\tNo ID\nokay\tLabel\nokay\tDuplicate\ncontrol\tbad\u{7}label\n");
        assert_eq!(models.len(),1);assert_eq!(models[0].id,"okay");
    }
    #[test]
    fn validates_cli_model_without_treating_it_as_an_option(){
        assert!(valid_model("gemini-3.8-flash"));
        for model in ["", "-dangerous", "model other", "model\nother"] {assert!(!valid_model(model));}
    }
    #[cfg(windows)]
    #[test]
    fn launches_cli_with_model_as_a_single_argument(){
        let command=launch_command(std::path::Path::new("C:\\agy.exe"),std::path::Path::new("C:\\work"),"model;still-an-argument");
        let args:Vec<_>=command.get_args().map(|arg|arg.to_string_lossy().into_owned()).collect();
        assert_eq!(args,["--model","model;still-an-argument"]);
    }
    #[cfg(windows)]
    #[test]
    fn login_uses_the_cli_interactive_login_flow(){
        let command=login_command(std::path::Path::new("C:\\agy.exe"));
        let args:Vec<_>=command.get_args().map(|arg|arg.to_string_lossy().into_owned()).collect();
        assert_eq!(args,["acp","--login"]);
    }
    #[tokio::test]
    #[ignore = "Uses the installed Antigravity CLI and its existing login"]
    async fn installed_cli_model_discovery(){assert!(!antigravity_cli_models().await.unwrap().is_empty());}
}
