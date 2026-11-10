import { useEffect, useState, type FormEvent } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { Copy } from 'lucide-react';
import gatewayIcon from './assets/summary-services.png';
import { parseSavedProviders, savedProvidersKey, type SavedProvider } from './saved-providers';
import { readAccounts } from './AntigravityCliModels';
import './gateway.css';

type Status = { address:string; token:string; base_url:string };
type Provider = { id:string; base_url:string; is_default:boolean; chat_only:boolean; anthropic_native:boolean; antigravity:boolean };
type OpenRouterUsage = { limit:number|null; remaining:number|null; usage:number|null; daily:number|null; weekly:number|null; monthly:number|null; reset:string|null };
type DeepSeekBalance = { available:boolean; balances:{currency:string;total:string;granted:string|null;topped_up:string|null}[] };
type GrokUsage = { used_percent:number|null; reset:string|null; period:string|null };
type CopilotUsage = { reset:string|null; windows:{kind:string;entitlement:number|null;remaining:number|null;percent_remaining:number|null;unlimited:boolean|null;overage_count:number|null;overage_permitted:boolean|null}[] };
type KiroUsage = {plan:string|null;reset_ms:number|null;windows:{kind:string;used:number|null;limit:number|null;overage:number|null}[]};
type KimiUsage = {windows:{kind:string;used:number|null;limit:number|null;remaining:number|null;used_ratio:number|null;reset:string|null;duration_minutes:number|null}[];wallet:{total_cents:number|null;remaining_cents:number|null;currency:string|null;monthly_limit_cents:number|null;monthly_used_cents:number|null;monthly_limit_enabled:boolean|null}|null};
type GlmQuota = {windows:{kind:string;unit:number|null;number:number|null;percent_used:number|null;used:number|null;limit:number|null;remaining:number|null;reset_ms:number|null}[]};
type ClineUsage = {windows:{kind:string;percent_used:number;reset:string|null}[]};
function formatClineReset(value:string):string { const date=new Date(value); return Number.isNaN(date.getTime())?'date indisponible':date.toLocaleString(); }
type OpenCodeGoUsage = {windows:{kind:string;status:string;percent_used:number;reset:string}[]};
const glmQuotaUrls=['https://api.z.ai/','https://api.z.ai/api/coding/paas/v4/','https://api.z.ai/api/anthropic/','https://open.bigmodel.cn/','https://open.bigmodel.cn/api/coding/paas/v4/','https://open.bigmodel.cn/api/v1/','https://open.bigmodel.cn/api/anthropic/'];
const presets = [
  { name:'OpenAI API', id:'openai', url:'https://api.openai.com/v1' },
  { name:'Anthropic API', id:'anthropic', url:'https://api.anthropic.com/v1' },
  { name:'Gemini API', id:'gemini', url:'https://generativelanguage.googleapis.com/v1beta/openai' },
  { name:'xAI API', id:'xai', url:'https://api.x.ai/v1' },
  { name:'GitHub Copilot · compte', id:'copilot', url:'https://api.githubcopilot.com' },
  { name:'OpenRouter', id:'openrouter', url:'https://openrouter.ai/api/v1' },
  { name:'Cline API', id:'cline', url:'https://api.cline.bot/api/v1' },
  { name:'ClinePass · abonnement', id:'cline-pass', url:'https://api.cline.bot/api/v1' },
  { name:'OpenCode Go · abonnement', id:'opencode-go', url:'https://opencode.ai/zen/go/v1' },
  { name:'OpenCode Zen · paiement à l’usage', id:'opencode-zen', url:'https://opencode.ai/zen/v1' },
  { name:'DeepSeek', id:'deepseek', url:'https://api.deepseek.com/v1' },
  { name:'Z.ai GLM API', id:'zai', url:'https://api.z.ai/api/paas/v4' },
  { name:'Z.ai GLM Coding Plan', id:'zai-code', url:'https://api.z.ai/api/coding/paas/v4' },
  { name:'BigModel GLM Coding Plan · Chine', id:'bigmodel-code', url:'https://open.bigmodel.cn/api/coding/paas/v4' },
  { name:'BigModel GLM Responses · Chine', id:'bigmodel-responses', url:'https://open.bigmodel.cn/api/v1' },
  { name:'Groq', id:'groq', url:'https://api.groq.com/openai/v1' },
  { name:'Mistral', id:'mistral', url:'https://api.mistral.ai/v1' },
  { name:'Together AI', id:'together', url:'https://api.together.xyz/v1' },
  { name:'Fireworks AI', id:'fireworks', url:'https://api.fireworks.ai/inference/v1' },
  { name:'Cerebras', id:'cerebras', url:'https://api.cerebras.ai/v1' },
  { name:'Hugging Face', id:'huggingface', url:'https://router.huggingface.co/v1' },
  { name:'NVIDIA NIM', id:'nvidia', url:'https://integrate.api.nvidia.com/v1' },
  { name:'DeepInfra', id:'deepinfra', url:'https://api.deepinfra.com/v1/openai' },
  { name:'Nous Research · clé API', id:'nous', url:'https://inference-api.nousresearch.com/v1' },
  { name:'Chutes', id:'chutes', url:'https://llm.chutes.ai/v1' },
  { name:'Hyperbolic', id:'hyperbolic', url:'https://api.hyperbolic.xyz/v1' },
  { name:'Nscale', id:'nscale', url:'https://inference.api.nscale.com/v1' },
  { name:'Vultr Inference', id:'vultr', url:'https://api.vultrinference.com/v1' },
  { name:'Command Code', id:'command-code', url:'https://api.commandcode.ai/provider/v1' },
  { name:'PackyCode', id:'packycode', url:'https://cf.api.fan/v1' },
  { name:'Baseten', id:'baseten', url:'https://inference.baseten.co/v1' },
  { name:'SambaNova Cloud', id:'sambanova', url:'https://api.sambanova.ai/v1' },
  { name:'Nebius Token Factory', id:'nebius', url:'https://api.tokenfactory.nebius.com/v1' },
  { name:'Crusoe', id:'crusoe', url:'https://api.inference.crusoecloud.com/v1' },
  { name:'DigitalOcean Inference', id:'digitalocean', url:'https://inference.do-ai.run/v1' },
  { name:'Scaleway', id:'scaleway', url:'https://api.scaleway.ai/v1' },
  { name:'Featherless AI', id:'featherless', url:'https://api.featherless.ai/v1' },
  { name:'Novita AI', id:'novita', url:'https://api.novita.ai/openai/v1' },
  { name:'Moonshot API', id:'moonshot', url:'https://api.moonshot.ai/v1' },
  { name:'Kimi Coding API', id:'kimi', url:'https://api.kimi.com/coding/v1' },
  { name:'Alibaba Model Studio API · Chine', id:'qwen', url:'https://dashscope.aliyuncs.com/compatible-mode/v1' },
  { name:'Kiro · AWS Builder ID', id:'kiro', url:'https://codewhisperer.us-east-1.amazonaws.com' },
  { name:'Qwen · compte avec abonnement', id:'qwen-account', url:'https://portal.qwen.ai/v1' },
  { name:'QwenCloud API · international', id:'qwen-cloud', url:'https://maas.qwencloudapi.com/compatible-mode/v1' },
  { name:'Alibaba Coding Plan · international', id:'alibaba-code-intl', url:'https://coding-intl.dashscope.aliyuncs.com/v1' },
  { name:'Alibaba Coding Plan · Chine', id:'alibaba-code-cn', url:'https://coding.dashscope.aliyuncs.com/v1' },
  { name:'Alibaba Token Plan · international', id:'alibaba-token-intl', url:'https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1' },
  { name:'Alibaba Token Plan · Chine', id:'alibaba-token-cn', url:'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1' },
  { name:'QwenCloud Token Plan', id:'qwen-token', url:'https://token-plan.maas.qwencloudapi.com/compatible-mode/v1' },
  { name:'Tencent Cloud Coding Plan', id:'tencent-code', url:'https://api.lkeap.cloud.tencent.com/coding/v3' },
  { name:'Volcengine Ark API', id:'volcengine', url:'https://ark.cn-beijing.volces.com/api/v3' },
  { name:'Volcengine Coding Plan', id:'volcengine-code', url:'https://ark.cn-beijing.volces.com/api/coding/v3' },
  { name:'Volcengine Agent Plan', id:'volcengine-agent', url:'https://ark.cn-beijing.volces.com/api/plan/v3' },
  { name:'SiliconFlow', id:'siliconflow', url:'https://api.siliconflow.cn/v1' },
  { name:'MiniMax', id:'minimax', url:'https://api.minimax.io/v1' },
  { name:'Xiaomi MiMo · Chat', id:'mimo', url:'https://api.xiaomimimo.com/v1' },
  { name:'OrcaRouter', id:'orcarouter', url:'https://api.orcarouter.ai/v1' },
  { name:'OpenGateway', id:'opengateway', url:'https://apis.opengateway.ai/v1' },
  { name:'Kilo Gateway', id:'kilo', url:'https://api.kilo.ai/api/gateway' },
  { name:'Opper', id:'opper', url:'https://api.opper.ai/v3/compat' },
  { name:'TokenLab', id:'tokenlab', url:'https://api.tokenlab.sh/v1' },
  { name:'Modelflare', id:'modelflare', url:'https://modelflare.dev/v1' },
  { name:'AI Stupid Level', id:'aistupidlevel', url:'https://aistupidlevel.info/v1' },
  { name:'Meta Model API', id:'meta', url:'https://api.meta.ai/v1' },
  { name:'GitLab Duo', id:'gitlab', url:'https://cloud.gitlab.com/ai/v1/proxy/openai/v1' },
  { name:'Ollama Cloud', id:'ollama-cloud', url:'https://ollama.com/v1' },
  { name:'Ollama local', id:'ollama', url:'http://127.0.0.1:11434/v1' },
  { name:'LM Studio local', id:'lmstudio', url:'http://127.0.0.1:1234/v1' },
  { name:'vLLM local', id:'vllm', url:'http://127.0.0.1:8000/v1' },
  { name:'Adresse personnalisée', id:'personnalise', url:'' },
];

export function Gateway() {
 const [provider,setProvider]=useState(presets[0].name);
 const [providerId,setProviderId]=useState(presets[0].id);
 const [chatOnly,setChatOnly]=useState(false);
 const [anthropicNative,setAnthropicNative]=useState(false);
 const [url,setUrl]=useState(presets[0].url);
 const [key,setKey]=useState('');
 const [remember,setRemember]=useState(false);
 const [saved,setSaved]=useState<SavedProvider[]>(()=>parseSavedProviders(localStorage.getItem(savedProvidersKey)));
 const [port,setPort]=useState('10101');
 const [status,setStatus]=useState<Status|null>(null);
 const [busy,setBusy]=useState(false);
 const [showToken,setShowToken]=useState(false);
 const [models,setModels]=useState<string[]|null>(null);
 const [providers,setProviders]=useState<Provider[]>([]);
 const [googleAccounts]=useState(readAccounts);
 const [googleAccount,setGoogleAccount]=useState(googleAccounts[0]?.id??'');
 const [googleId,setGoogleId]=useState('antigravity');
 const [routerUsage,setRouterUsage]=useState<{id:string;data:OpenRouterUsage}|null>(null);
 const [deepseekBalance,setDeepseekBalance]=useState<{id:string;data:DeepSeekBalance}|null>(null);
 const [grokUsage,setGrokUsage]=useState<{id:string;data:GrokUsage}|null>(null);
 const [copilotUsage,setCopilotUsage]=useState<{id:string;data:CopilotUsage}|null>(null);
 const [kiroUsage,setKiroUsage]=useState<{id:string;data:KiroUsage}|null>(null);
 const [kimiUsage,setKimiUsage]=useState<{id:string;data:KimiUsage}|null>(null);
 const [glmQuota,setGlmQuota]=useState<{id:string;data:GlmQuota}|null>(null);
 const [clineUsage,setClineUsage]=useState<{id:string;data:ClineUsage}|null>(null);
 const [openCodeUsage,setOpenCodeUsage]=useState<{id:string;data:OpenCodeGoUsage}|null>(null);
 const [selectedProvider,setSelectedProvider]=useState('');
 const [loadingModels,setLoadingModels]=useState(false);
 const [launchModel,setLaunchModel]=useState('');
 const [workspace,setWorkspace]=useState('');
 const [message,setMessage]=useState('');
 const [nousPrompt,setNousPrompt]=useState<{user_code:string;verification_uri:string;providerId:string}|null>(null);
 const [nousPolling,setNousPolling]=useState(false);
 const [kiroPrompt,setKiroPrompt]=useState<{user_code:string;verification_uri:string;providerId:string}|null>(null);
 const [kiroPolling,setKiroPolling]=useState(false);
 const [qwenPrompt,setQwenPrompt]=useState<{user_code:string;verification_uri:string;providerId:string}|null>(null);
 const [qwenPolling,setQwenPolling]=useState(false);
 const [kimiPrompt,setKimiPrompt]=useState<{user_code:string;verification_uri:string;providerId:string}|null>(null);
 const [kimiPolling,setKimiPolling]=useState(false);
 const [copilotPrompt,setCopilotPrompt]=useState<{user_code:string;verification_uri:string;providerId:string}|null>(null);
 const [copilotPolling,setCopilotPolling]=useState(false);
 useEffect(()=>{if(isTauri())void invoke<Status|null>('gateway_status').then(async current=>{setStatus(current);if(current){setPort(new URL(current.address).port);const list=await invoke<Provider[]>('gateway_providers');setProviders(list);setSelectedProvider(list.find(p=>p.is_default)?.id??'')}}).catch(()=>setMessage('Impossible de lire l’état de la passerelle.'))},[]);
 useEffect(()=>{setRouterUsage(null);setDeepseekBalance(null);setGlmQuota(null);setOpenCodeUsage(null);setClineUsage(null)},[providers]);
 async function start(event:FormEvent){
  event.preventDefault();setMessage('');
  const number=Number(port);
  if(!Number.isInteger(number)||number<1||number>65535){setMessage('Choisis un port entre 1 et 65535.');return}
  if(!isTauri()){setMessage('Ouvre l’application Windows pour utiliser la passerelle.');return}
  if(remember&&saved.some(item=>item.id===providerId)){setMessage('Cet identifiant est déjà enregistré. Oublie-le d’abord ou choisis un autre identifiant.');return}
  setBusy(true);
  try{
   if(status){const list=await invoke<Provider[]>('gateway_add_provider',{providerId,baseUrl:url,apiKey:key,credentialRef:null,chatOnly,anthropicNative});setProviders(list);setSelectedProvider(providerId)}
   else{const current=await invoke<Status>('gateway_start',{providerId,baseUrl:url,apiKey:key,credentialRef:null,chatOnly,anthropicNative,port:number});setStatus(current);const list=await invoke<Provider[]>('gateway_providers');setProviders(list);setSelectedProvider(providerId)}
   if(remember){
    let credentialRef:string|null=null;
    try{
     if(key)credentialRef=await invoke<string>('gateway_remember_key',{baseUrl:url,apiKey:key,chatOnly,anthropicNative});
     const next=[...saved,{id:providerId,baseUrl:url,chatOnly,anthropicNative,credentialRef}];
     localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);setMessage('Fournisseur enregistré sur cet appareil.');
    }catch{
     if(credentialRef){try{await invoke('gateway_forget_key',{reference:credentialRef})}catch{/* La clé pourra être retirée depuis le coffre Windows. */}}
     setMessage('Le fournisseur est actif, mais son enregistrement a échoué.');
    }
   }
   setKey('');setModels(null)
  }
  catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function stop(){setMessage('');setBusy(true);try{await invoke('gateway_stop');setStatus(null);setModels(null);setProviders([]);setSelectedProvider('')}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function removeProvider(id:string){setMessage('');setBusy(true);try{const list=await invoke<Provider[]>('gateway_remove_provider',{providerId:id});setProviders(list);if(selectedProvider===id){setSelectedProvider(list.find(p=>p.is_default)?.id??'');setModels(null)}}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function activateSaved(item:SavedProvider){
  setMessage('');if(!isTauri()){setMessage('Ouvre l’application Windows pour utiliser la passerelle.');return}
  const number=Number(port);if(!Number.isInteger(number)||number<1||number>65535){setMessage('Choisis un port entre 1 et 65535.');return}
  setBusy(true);
  try{
   const args={providerId:item.id,baseUrl:item.baseUrl,apiKey:'',credentialRef:item.credentialRef,chatOnly:item.chatOnly,anthropicNative:item.anthropicNative};
   if(item.authKind==='kimi'||item.authKind==='xai'||item.authKind==='copilot'||item.authKind==='chatgpt'||item.authKind==='qwen'||item.authKind==='nous'||item.authKind==='kiro'){
    if(!item.credentialRef)throw new Error('Connexion au compte manquante.');
    const accountArgs={providerId:item.id,credentialRef:item.credentialRef};
    if(status)setProviders(await invoke<Provider[]>(`gateway_add_${item.authKind}`,accountArgs));
    else{setStatus(await invoke<Status>(`gateway_start_${item.authKind}`,{...accountArgs,port:number}));setProviders(await invoke<Provider[]>('gateway_providers'))}
   }else if(status){setProviders(await invoke<Provider[]>('gateway_add_provider',args))}
   else{setStatus(await invoke<Status>('gateway_start',{...args,port:number}));setProviders(await invoke<Provider[]>('gateway_providers'))}
   setSelectedProvider(item.id);setModels(null);
  }catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function forgetSaved(item:SavedProvider){
  setMessage('');setBusy(true);
  try{
   let revoked=true;if(item.credentialRef){if(item.authKind==='chatgpt')revoked=await invoke<boolean>('chatgpt_plan_forget',{reference:item.credentialRef});else await invoke('gateway_forget_key',{reference:item.credentialRef})}
   const next=saved.filter(value=>value.id!==item.id);localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);
   if(grokUsage?.id===item.id)setGrokUsage(null);
   if(copilotUsage?.id===item.id)setCopilotUsage(null);
   if(kimiUsage?.id===item.id)setKimiUsage(null);
   if(kiroUsage?.id===item.id)setKiroUsage(null);
   setMessage(`Fournisseur oublié sur cet appareil.${item.authKind==='chatgpt'&&!revoked?' La révocation chez OpenAI n’a pas été confirmée : vérifie les applications connectées dans ChatGPT.':''}${item.baseUrl.replace(/\/$/,'')==='https://openrouter.ai/api/v1'?' La clé reste active chez OpenRouter : révoque-la dans ton compte si nécessaire.':''} S’il est actif, retire-le ou arrête la passerelle.`);
  }catch(error){setMessage(`Impossible d’oublier ce fournisseur : ${String(error)}`)}finally{setBusy(false)}
 }
 async function loadModels(){setMessage('');setLoadingModels(true);try{setModels(await invoke<string[]>('gateway_models',{providerId:selectedProvider}))}catch(error){setMessage(String(error))}finally{setLoadingModels(false)}}
 async function addAntigravity(){
  const account=googleAccounts.find(item=>item.id===googleAccount);if(!account)return;
  if(!isTauri()){setMessage('Ouvre l’application Windows pour utiliser la passerelle.');return}
  const number=Number(port);if(!Number.isInteger(number)||number<1||number>65535){setMessage('Choisis un port entre 1 et 65535.');return}
  setMessage('');setBusy(true);
  try{
   const args={providerId:googleId,credentialRef:account.credential_ref,accountId:account.id};
   if(status){setProviders(await invoke<Provider[]>('gateway_add_antigravity',args))}
   else{setStatus(await invoke<Status>('gateway_start_antigravity',{...args,port:number}));setProviders(await invoke<Provider[]>('gateway_providers'))}
   setSelectedProvider(googleId);setModels(null);setMessage('Compte actif. Les conversations texte sont prises en charge. Les outils simples sont expérimentaux en Chat, Responses et Messages ; Chat et Responses diffusent le texte progressivement sans outils ; avec outils, le flux attend la réponse complète.');
  }catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function loadRouterUsage(id:string){setMessage('');setRouterUsage(null);setBusy(true);try{setRouterUsage({id,data:await invoke<OpenRouterUsage>('gateway_openrouter_usage',{providerId:id})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadDeepseekBalance(id:string){setMessage('');setDeepseekBalance(null);setBusy(true);try{setDeepseekBalance({id,data:await invoke<DeepSeekBalance>('gateway_deepseek_balance',{providerId:id})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadGrokUsage(item:SavedProvider){if(!item.credentialRef)return;setMessage('');setGrokUsage(null);setBusy(true);try{setGrokUsage({id:item.id,data:await invoke<GrokUsage>('xai_usage',{reference:item.credentialRef})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadCopilotUsage(item:SavedProvider){if(!item.credentialRef)return;setMessage('');setCopilotUsage(null);setBusy(true);try{setCopilotUsage({id:item.id,data:await invoke<CopilotUsage>('copilot_usage',{reference:item.credentialRef})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadKiroUsage(item:SavedProvider){if(!item.credentialRef)return;setMessage('');setKiroUsage(null);setBusy(true);try{setKiroUsage({id:item.id,data:await invoke<KiroUsage>('kiro_builder_usage',{credentialRef:item.credentialRef})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadKimiUsage(item:SavedProvider){if(!item.credentialRef)return;setMessage('');setKimiUsage(null);setBusy(true);try{setKimiUsage({id:item.id,data:await invoke<KimiUsage>('kimi_usage',{reference:item.credentialRef})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadGlmQuota(id:string){setMessage('');setGlmQuota(null);setBusy(true);try{setGlmQuota({id,data:await invoke<GlmQuota>('gateway_glm_quota',{providerId:id})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadClineUsage(id:string){setMessage('');setClineUsage(null);setBusy(true);try{setClineUsage({id,data:await invoke<ClineUsage>('gateway_cline_usage',{providerId:id})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function loadOpenCodeUsage(id:string){setMessage('');setOpenCodeUsage(null);setBusy(true);try{setOpenCodeUsage({id,data:await invoke<OpenCodeGoUsage>('gateway_opencode_go_usage',{providerId:id})})}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function startNous(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter Nous.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)||saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Choisis un identifiant de routage disponible.');return}
  setBusy(true);setMessage('');try{const prompt=await invoke<{user_code:string;verification_uri:string}>('nous_login_start');setNousPrompt({...prompt,providerId});setMessage('Ouvre Nous Portal, saisis le code puis reviens dans Nivra.')}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function finishNous(){
  if(!nousPrompt)return;setBusy(true);setNousPolling(true);setMessage('En attente de l’autorisation Nous…');let reference:string|null=null;
  try{reference=await invoke<string>('nous_login_finish');const next:SavedProvider[]=[...saved,{id:nousPrompt.providerId,baseUrl:'https://inference-api.nousresearch.com/v1',chatOnly:true,anthropicNative:false,credentialRef:reference,authKind:'nous'}];localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);setNousPrompt(null);reference=null;setMessage('Compte Nous enregistré. Active-le dans les fournisseurs enregistrés.');}
  catch(error){if(reference){try{await invoke('gateway_forget_key',{reference})}catch{/* Référence temporaire conservée dans le coffre Windows. */}}setNousPrompt(null);setMessage(String(error))}finally{setNousPolling(false);setBusy(false)}
 }
 async function cancelNous(){try{await invoke('nous_login_cancel');setNousPrompt(null);setMessage('Connexion Nous annulée.')}catch(error){setMessage(String(error))}}
 async function startQwen(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter Qwen.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)||saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Choisis un identifiant de routage disponible.');return}
  setBusy(true);setMessage('');try{const prompt=await invoke<{user_code:string;verification_uri:string}>('qwen_login_start');setQwenPrompt({...prompt,providerId});setMessage('Ouvre la page Qwen, saisis le code puis reviens dans Nivra.')}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function finishQwen(){
  if(!qwenPrompt)return;setBusy(true);setQwenPolling(true);setMessage('En attente de l’autorisation Qwen…');let reference:string|null=null;
  try{reference=await invoke<string>('qwen_login_finish');const next:SavedProvider[]=[...saved,{id:qwenPrompt.providerId,baseUrl:'https://portal.qwen.ai/v1',chatOnly:true,anthropicNative:false,credentialRef:reference,authKind:'qwen'}];localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);setQwenPrompt(null);reference=null;setMessage('Compte Qwen enregistré. Active-le dans les fournisseurs enregistrés.');}
  catch(error){if(reference){try{await invoke('gateway_forget_key',{reference})}catch{/* Référence temporaire conservée dans le coffre Windows. */}}setQwenPrompt(null);setMessage(String(error))}finally{setQwenPolling(false);setBusy(false)}
 }
 async function cancelQwen(){try{await invoke('qwen_login_cancel');setQwenPrompt(null);setMessage('Connexion Qwen annulée.')}catch(error){setMessage(String(error))}}
 async function startKiro(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter Kiro.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)||saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Choisis un identifiant de routage disponible.');return}
  setBusy(true);setMessage('');try{const prompt=await invoke<{user_code:string;verification_uri:string}>('kiro_builder_start');setKiroPrompt({...prompt,providerId});setMessage('Ouvre la page Kiro, saisis le code puis reviens dans Nivra.')}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function finishKiro(){
  if(!kiroPrompt)return;setBusy(true);setKiroPolling(true);setMessage('En attente de l’autorisation Kiro…');let reference:string|null=null;
  try{reference=await invoke<string>('kiro_builder_finish');const next:SavedProvider[]=[...saved,{id:kiroPrompt.providerId,baseUrl:'https://codewhisperer.us-east-1.amazonaws.com',chatOnly:true,anthropicNative:false,credentialRef:reference,authKind:'kiro'}];localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);setKiroPrompt(null);reference=null;setMessage('Compte Kiro enregistré. Active-le dans les fournisseurs enregistrés.');}
  catch(error){if(reference){try{await invoke('gateway_forget_key',{reference})}catch{/* Référence temporaire conservée dans le coffre Windows. */}}setKiroPrompt(null);setMessage(String(error))}finally{setKiroPolling(false);setBusy(false)}
 }
 async function cancelKiro(){try{await invoke('kiro_builder_cancel');setKiroPrompt(null);setMessage('Connexion Kiro annulée.')}catch(error){setMessage(String(error))}}
 async function startKimi(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter Kimi.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)||saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Choisis un identifiant de routage disponible.');return}
  setBusy(true);setMessage('');try{const prompt=await invoke<{user_code:string;verification_uri:string}>('kimi_login_start');setKimiPrompt({...prompt,providerId});setMessage('Ouvre la page Kimi, saisis le code puis reviens dans Nivra.')}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function finishKimi(){
  if(!kimiPrompt)return;setBusy(true);setKimiPolling(true);setMessage('En attente de l’autorisation Kimi…');let reference:string|null=null;
  try{
   reference=await invoke<string>('kimi_login_finish');
   const next:SavedProvider[]=[...saved,{id:kimiPrompt.providerId,baseUrl:'https://api.kimi.com/coding/v1',chatOnly:true,anthropicNative:false,credentialRef:reference,authKind:'kimi'}];
   localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);setKimiPrompt(null);reference=null;setMessage('Compte Kimi enregistré. Active-le dans les fournisseurs enregistrés.');
  }catch(error){if(reference){try{await invoke('gateway_forget_key',{reference})}catch{/* Référence temporaire conservée dans le coffre Windows. */}}setKimiPrompt(null);setMessage(String(error))}finally{setKimiPolling(false);setBusy(false)}
 }
 async function cancelKimi(){try{await invoke('kimi_login_cancel');setKimiPrompt(null);setMessage('Connexion Kimi annulée.')}catch(error){setMessage(String(error))}}

 async function startCopilot(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter GitHub Copilot.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)||saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Choisis un identifiant de routage disponible.');return}
  setBusy(true);setMessage('');try{const prompt=await invoke<{user_code:string;verification_uri:string}>('copilot_login_start');setCopilotPrompt({...prompt,providerId});setMessage('Ouvre GitHub, saisis le code puis reviens dans Nivra.')}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function finishCopilot(){
  if(!copilotPrompt)return;setBusy(true);setCopilotPolling(true);setMessage('En attente de l’autorisation GitHub…');let reference:string|null=null;
  try{reference=await invoke<string>('copilot_login_finish');const next:SavedProvider[]=[...saved,{id:copilotPrompt.providerId,baseUrl:'https://api.githubcopilot.com',chatOnly:true,anthropicNative:false,credentialRef:reference,authKind:'copilot'}];localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);setCopilotPrompt(null);reference=null;setMessage('Compte Copilot enregistré. Active-le dans les fournisseurs enregistrés.');}
  catch(error){if(reference){try{await invoke('gateway_forget_key',{reference})}catch{/* Référence temporaire conservée dans le coffre Windows. */}}setCopilotPrompt(null);setMessage(String(error))}finally{setCopilotPolling(false);setBusy(false)}
 }
 async function cancelCopilot(){try{await invoke('copilot_login_cancel');setCopilotPrompt(null);setMessage('Connexion GitHub annulée.')}catch(error){setMessage(String(error))}}

 async function connectXai(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter Grok.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)||saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Choisis un identifiant de routage disponible.');return}
  setBusy(true);setMessage('Autorise la connexion xAI dans ton navigateur.');let reference:string|null=null;
  try{
   reference=await invoke<string>('xai_login');
   const next:SavedProvider[]=[...saved,{id:providerId,baseUrl:'https://cli-chat-proxy.grok.com/v1',chatOnly:true,anthropicNative:false,credentialRef:reference,authKind:'xai'}];
   localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);reference=null;setMessage('Compte Grok enregistré. Active-le dans les fournisseurs enregistrés.');
  }catch(error){if(reference){try{await invoke('gateway_forget_key',{reference})}catch{/* Référence temporaire conservée dans le coffre Windows. */}}setMessage(String(error))}finally{setBusy(false)}
 }

 async function connectChatgpt(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter ChatGPT.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)||saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Choisis un identifiant de routage disponible.');return}
  setBusy(true);setMessage('Autorise Nivra à utiliser ton abonnement dans le navigateur.');let reference:string|null=null;
  try{const result=await invoke<{credential_ref:string;email:string|null}>('chatgpt_plan_login');reference=result.credential_ref;const next:SavedProvider[]=[...saved,{id:providerId,baseUrl:'https://api.openai.com/v1',chatOnly:false,anthropicNative:false,credentialRef:reference,authKind:'chatgpt'}];localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);reference=null;setMessage(`Compte ChatGPT${result.email?` ${result.email}`:''} enregistré. Active-le dans les fournisseurs enregistrés.`)}
  catch(error){if(reference){try{await invoke('chatgpt_plan_forget',{reference})}catch{/* La connexion pourra être retirée dans les réglages ChatGPT. */}}setMessage(String(error))}finally{setBusy(false)}
 }

 async function connectOpenRouter(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter OpenRouter.');return}
  if(!/^[a-z0-9_-]{1,40}$/.test(providerId)){setMessage('Choisis un identifiant de routage valide.');return}
  if(saved.some(item=>item.id===providerId)||providers.some(item=>item.id===providerId)){setMessage('Cet identifiant est déjà utilisé. Choisis-en un autre.');return}
  setMessage('Autorise Nivra dans ton navigateur. La connexion peut prendre quelques instants.');setBusy(true);
  let credentialRef:string|null=null;
  try{
   credentialRef=await invoke<string>('openrouter_login');
   const next=[...saved,{id:providerId,baseUrl:'https://openrouter.ai/api/v1',chatOnly:true,anthropicNative:false,credentialRef}];
   localStorage.setItem(savedProvidersKey,JSON.stringify(next));setSaved(next);setMessage('OpenRouter connecté et enregistré. Active le fournisseur enregistré pour utiliser la passerelle.');
  }catch(error){if(credentialRef){try{await invoke('gateway_forget_key',{reference:credentialRef})}catch{/* La clé peut être retirée du coffre Windows. */}setMessage('La clé a été créée chez OpenRouter, mais Nivra n’a pas pu enregistrer le fournisseur. Vérifie ou révoque cette clé dans OpenRouter avant de réessayer.')}else setMessage(String(error))}finally{setBusy(false)}
 }
 async function launchCodex(event:FormEvent){event.preventDefault();setMessage('');setBusy(true);try{await invoke('gateway_launch_codex',{model:launchModel,workspace});setMessage('Ouverture de Codex demandée. Garde Nivra et la passerelle ouverts pendant la session.')}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function launchClaude(){setMessage('');setBusy(true);try{await invoke('gateway_launch_claude',{model:launchModel,workspace});setMessage('Ouverture de Claude Code demandée. Garde Nivra et la passerelle ouverts pendant la session.')}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function copy(value:string){try{await navigator.clipboard.writeText(value);setMessage('Copié.')}catch{setMessage('Impossible de copier automatiquement ; sélectionne le texte.') }}
 return <section className="gateway-page">
  <header className="page-head"><div className="page-heading"><span className="gateway-heading-icon"><img src={gatewayIcon} alt=""/></span><div><h1>Passerelle</h1><p>Connecte Codex ou Claude Code à tes fournisseurs.</p></div></div><span className={`gateway-status ${status?'running':''}`}>{status?'Active':'Arrêtée'}</span></header>
  <div className="gateway-panel"><h2>{status?'Ajouter un fournisseur':'Démarrer avec un fournisseur'}</h2><p>Ces préréglages utilisent une clé API ou un serveur local. Ils ne connectent pas automatiquement un abonnement ou un compte. Sans enregistrement, les clés restent uniquement en mémoire pendant la session.</p>
   <form onSubmit={start} className="gateway-form">
    <label>Fournisseur<select value={provider} disabled={busy} onChange={e=>{const value=e.target.value;const preset=presets.find(p=>p.name===value);setProvider(value);setKey('');setUrl(preset?.url??'');setProviderId(preset?.id??'');setAnthropicNative(value==='Anthropic API');setChatOnly(!['OpenAI API','Anthropic API','xAI API','BigModel GLM Responses · Chine'].includes(value))}}>{presets.map(p=><option key={p.name}>{p.name}</option>)}</select></label>
    {(provider==='Cline API'||provider==='ClinePass · abonnement')&&<p>Utilise une clé API Cline. La liste montre les modèles recommandés par Cline, mais leur présence ne garantit pas l’accès avec ton compte. Pour ClinePass, conserve le nom complet du modèle, par exemple <code>cline-pass/nom-du-modèle</code>.</p>}
    <label>Identifiant de routage<input value={providerId} disabled={busy} required pattern="[a-z0-9_-]{1,40}" onChange={e=>setProviderId(e.target.value)} placeholder="ex. openai"/></label>
    {provider==='OpenAI API'&&<><p>Tu peux aussi autoriser Nivra à utiliser ton abonnement ChatGPT. Connexion expérimentale, indépendante de tes profils Codex.</p><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void connectChatgpt()}>Continuer avec ChatGPT</button></>}
    {provider==='xAI API'&&<><p>La connexion par compte Grok utilise le service de l’abonnement. Fonction expérimentale, à vérifier avec ton compte.</p><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void connectXai()}>Connecter un compte Grok</button></>}
    {provider==='Nous Research · clé API'&&<><p>Utilise une clé API ou connecte ton compte Nous Portal. Connexion par compte expérimentale, à vérifier avec ton offre.</p>{!nousPrompt?<button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void startNous()}>Connecter Nous Portal</button>:<div role="status"><p>Ouvre <a href={nousPrompt.verification_uri} target="_blank" rel="noreferrer">Nous Portal</a>, puis saisis ce code : <strong>{nousPrompt.user_code}</strong></p><button type="button" className="button ghost" disabled={busy} onClick={()=>void copy(nousPrompt.user_code)}>Copier le code</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void finishNous()}>J’ai autorisé Nous</button><button type="button" className="button ghost" disabled={busy&&!nousPolling} onClick={()=>void cancelNous()}>Annuler</button></div>}</>}
    {provider.startsWith('Alibaba ')&&<p>Utilise la clé correspondant à cette offre et à cette région. Coding Plan, Token Plan et API à l’usage utilisent des adresses distinctes.</p>}
    {provider==='GitHub Copilot · compte'&&<><p>Connexion expérimentale non officielle : un abonnement Copilot actif peut être nécessaire.</p>{!copilotPrompt?<button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void startCopilot()}>Connecter GitHub Copilot</button>:<div role="status"><p>Ouvre <a href={copilotPrompt.verification_uri} target="_blank" rel="noreferrer">la page GitHub</a>, puis saisis ce code : <strong>{copilotPrompt.user_code}</strong></p><button type="button" className="button ghost" disabled={busy} onClick={()=>void copy(copilotPrompt.user_code)}>Copier le code</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void finishCopilot()}>J’ai autorisé GitHub</button><button type="button" className="button ghost" disabled={busy&&!copilotPolling} onClick={()=>void cancelCopilot()}>Annuler</button></div>}</>}
    {provider==='Qwen · compte avec abonnement'&&<><p>Connexion expérimentale à un compte Qwen avec abonnement compatible ; l’accès aux modèles dépend du compte.</p>{!qwenPrompt?<button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void startQwen()}>Connecter Qwen</button>:<div role="status"><p>Ouvre <a href={qwenPrompt.verification_uri} target="_blank" rel="noreferrer">la page Qwen</a>, puis saisis ce code : <strong>{qwenPrompt.user_code}</strong></p><button type="button" className="button ghost" disabled={busy} onClick={()=>void copy(qwenPrompt.user_code)}>Copier le code</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void finishQwen()}>J’ai autorisé Qwen</button><button type="button" className="button ghost" disabled={busy&&!qwenPolling} onClick={()=>void cancelQwen()}>Annuler</button></div>}</>}
    {provider==='Kiro · AWS Builder ID'&&<><p>Connexion expérimentale avec AWS Builder ID. La compatibilité et les modèles dépendent du compte. Les réponses en flux arrivent après génération complète.</p>{!kiroPrompt?<button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void startKiro()}>Connecter Kiro</button>:<div role="status"><p>Ouvre <a href={kiroPrompt.verification_uri} target="_blank" rel="noreferrer">la page Kiro</a>, puis saisis ce code : <strong>{kiroPrompt.user_code}</strong></p><button type="button" className="button ghost" disabled={busy} onClick={()=>void copy(kiroPrompt.user_code)}>Copier le code</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void finishKiro()}>J’ai autorisé Kiro</button><button type="button" className="button ghost" disabled={busy&&!kiroPolling} onClick={()=>void cancelKiro()}>Annuler</button></div>}</>}
    {provider==='Kimi Coding API'&&<><p>Connecte ton compte Kimi Code avec un code. Le renouvellement est conservé dans le coffre Windows.</p>{!kimiPrompt?<button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void startKimi()}>Connecter Kimi Code</button>:<div role="status"><p>Ouvre <a href={kimiPrompt.verification_uri} target="_blank" rel="noreferrer">la page de connexion Kimi</a>, puis saisis ce code : <strong>{kimiPrompt.user_code}</strong></p><button type="button" className="button ghost" disabled={busy} onClick={()=>void copy(kimiPrompt.verification_uri)}>Copier l’adresse</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void copy(kimiPrompt.user_code)}>Copier le code</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void finishKimi()}>J’ai autorisé Kimi</button><button type="button" className="button ghost" disabled={busy&&!kimiPolling} onClick={()=>void cancelKimi()}>Annuler</button></div>}</>}
    {provider==='OpenRouter'&&<><p>Connecte ton compte dans le navigateur : OpenRouter créera une clé pour Nivra, enregistrée dans le coffre Windows.</p><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void connectOpenRouter()}>Connecter OpenRouter</button></>}
    {provider!=='GitHub Copilot · compte'&&provider!=='Qwen · compte avec abonnement'&&provider!=='Kiro · AWS Builder ID'&&<><label>Adresse API<input type="url" value={url} disabled={busy} required onChange={e=>setUrl(e.target.value)} placeholder="https://…/v1"/></label>
    <label>Clé API du fournisseur <span>(facultative pour Ollama)</span><input type="password" value={key} disabled={busy} autoComplete="off" onChange={e=>setKey(e.target.value)} placeholder="Clé API"/></label>
    <label>Protocole du fournisseur<select value={anthropicNative?'anthropic':chatOnly?'chat':'responses'} disabled={busy} onChange={e=>{setChatOnly(e.target.value==='chat');setAnthropicNative(e.target.value==='anthropic')}}><option value="responses">Responses natif</option><option value="chat">Chat Completions · traduction Responses</option><option value="anthropic">Anthropic Messages natif</option></select></label>
    </>}
    <label>Port local<input type="number" value={port} disabled={!!status||busy} min="1" max="65535" onChange={e=>setPort(e.target.value)}/></label>
    {provider!=='GitHub Copilot · compte'&&provider!=='Qwen · compte avec abonnement'&&provider!=='Kiro · AWS Builder ID'&&<>
    <label className="gateway-remember"><input type="checkbox" checked={remember} disabled={busy} onChange={e=>setRemember(e.target.checked)}/><span>Enregistrer ce fournisseur sur cet appareil et conserver sa clé dans le coffre Windows</span></label>
    <button className="button primary" disabled={busy||loadingModels}>{status?'Ajouter le fournisseur':'Démarrer la passerelle'}</button></>}
   </form>
  </div>
  {saved.length>0&&<div className="gateway-panel"><h2>Fournisseurs enregistrés</h2><p>Active un fournisseur enregistré sans ressaisir sa clé. Oublier retire sa clé du coffre Windows.</p><ul className="gateway-models">{saved.map(item=><li key={item.id}><code>{item.id}</code>{item.authKind==='kimi'&&<span>Compte Kimi Code</span>}{item.authKind==='xai'&&<span>Compte Grok</span>}{item.authKind==='copilot'&&<span>Compte GitHub Copilot</span>}{item.authKind==='qwen'&&<span>Compte Qwen · expérimental</span>}{item.authKind==='kiro'&&<span>Compte Kiro · expérimental</span>}{item.authKind==='nous'&&<span>Compte Nous · expérimental</span>}{item.authKind==='chatgpt'&&<span>Compte ChatGPT · expérimental</span>}<button type="button" className="button ghost" disabled={busy||loadingModels||providers.some(active=>active.id===item.id)} onClick={()=>void activateSaved(item)}>Activer</button><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void forgetSaved(item)}>Oublier</button></li>)}</ul></div>}
  {saved.some(item=>item.authKind==='kiro')&&<div className="gateway-panel"><h2>Quotas Kiro</h2><p>Lis les crédits et limites transmis par Kiro pour ton compte AWS Builder ID.</p><ul className="gateway-models">{saved.filter(item=>item.authKind==='kiro').map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadKiroUsage(item)}>Lire les quotas</button></li>)}</ul>{kiroUsage&&<div role="status"><p><strong>{kiroUsage.id}</strong>{kiroUsage.data.plan?` · ${kiroUsage.data.plan}`:''}{kiroUsage.data.reset_ms!==null?` · Renouvellement : ${new Date(kiroUsage.data.reset_ms).toLocaleString()}`:''}</p>{kiroUsage.data.windows.length?kiroUsage.data.windows.map((window,index)=><p key={`${window.kind}-${index}`}><strong>{window.kind}</strong>{window.used!==null?` · Utilisé : ${window.used}`:''}{window.limit!==null?` · Limite : ${window.limit}`:''}{window.overage!==null?` · Dépassement : ${window.overage}`:''}</p>):<p>Kiro n’a communiqué aucun quota pour ce compte.</p>}</div>}</div>}
  {saved.some(item=>item.authKind==='kimi')&&<div className="gateway-panel"><h2>Quotas Kimi Code</h2><p>Consulte les données envoyées par Kimi. Les détails et les résumés sont affichés séparément : leurs valeurs peuvent différer.</p><ul className="gateway-models">{saved.filter(item=>item.authKind==='kimi').map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadKimiUsage(item)}>Lire les quotas</button></li>)}</ul>{kimiUsage&&<div role="status"><p><strong>{kimiUsage.id}</strong></p>{kimiUsage.data.windows.length?kimiUsage.data.windows.map((window,index)=><p key={`${window.kind}-${index}`}><strong>{window.kind==='fenetre'?`Détail${window.duration_minutes!==null?` · ${window.duration_minutes} min`:''}`:({principal:'Quota principal',total:'Quota total', '5h':'Résumé · 5 heures','7j':'Résumé · 7 jours',mois_total:'Résumé · mois total',mois_code:'Résumé · mois code'} as Record<string,string>)[window.kind]??window.kind}</strong>{window.used_ratio!==null?` · ${(window.used_ratio*100).toFixed(1)} % utilisé`:''}{window.used!==null?` · Utilisé : ${window.used}`:''}{window.limit!==null?` · Limite : ${window.limit}`:''}{window.remaining!==null?` · Restant : ${window.remaining}`:''}{window.reset?` · Renouvellement : ${window.reset}`:''}</p>):!kimiUsage.data.wallet?<p>Kimi n’a communiqué aucun quota pour ce compte.</p>:null}{kimiUsage.data.wallet&&<p><strong>Crédit supplémentaire</strong>{kimiUsage.data.wallet.remaining_cents!==null?` · Restant : ${(kimiUsage.data.wallet.remaining_cents/100).toFixed(2)} ${kimiUsage.data.wallet.currency??'(devise non communiquée)'}`:''}{kimiUsage.data.wallet.total_cents!==null?` · Total : ${(kimiUsage.data.wallet.total_cents/100).toFixed(2)}`:''}{kimiUsage.data.wallet.monthly_used_cents!==null?` · Utilisé ce mois : ${(kimiUsage.data.wallet.monthly_used_cents/100).toFixed(2)}`:''}{kimiUsage.data.wallet.monthly_limit_enabled===true&&kimiUsage.data.wallet.monthly_limit_cents!==null?` · Plafond mensuel : ${(kimiUsage.data.wallet.monthly_limit_cents/100).toFixed(2)}`:''}</p>}</div>}</div>}
  {saved.some(item=>item.authKind==='xai')&&<div className="gateway-panel"><h2>Quotas Grok</h2><p>Lecture expérimentale des limites communiquées par Grok ; les données varient selon ton offre.</p><ul className="gateway-models">{saved.filter(item=>item.authKind==='xai').map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadGrokUsage(item)}>Lire les quotas</button></li>)}</ul>{grokUsage&&<div role="status"><p><strong>{grokUsage.id}</strong>{grokUsage.data.period?` · Période ${grokUsage.data.period}`:''}</p><p>{grokUsage.data.used_percent!==null?`${grokUsage.data.used_percent} % utilisé · ${Math.round((100-grokUsage.data.used_percent)*100)/100} % restant`:'Pourcentage non communiqué'}{grokUsage.data.reset?` · Renouvellement : ${grokUsage.data.reset}`:''}</p></div>}</div>}
  {saved.some(item=>item.authKind==='copilot')&&<div className="gateway-panel"><h2>Quotas GitHub Copilot</h2><p>Lis les données communiquées par GitHub pour chaque compte enregistré. Certaines informations peuvent être indisponibles selon ton offre.</p><ul className="gateway-models">{saved.filter(item=>item.authKind==='copilot').map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadCopilotUsage(item)}>Lire les quotas</button></li>)}</ul>{copilotUsage&&<div role="status"><p><strong>{copilotUsage.id}</strong>{copilotUsage.data.reset?` · Renouvellement : ${copilotUsage.data.reset}`:''}</p>{copilotUsage.data.windows.length?copilotUsage.data.windows.map(window=><p key={window.kind}><strong>{({premium_interactions:'Requêtes premium',chat:'Chat',completions:'Complétions'} as Record<string,string>)[window.kind]??window.kind}</strong> · {window.unlimited===true?'Illimité':window.remaining!==null?`${window.remaining} restant${window.remaining===1?'':'s'}${window.entitlement!==null?` sur ${window.entitlement}`:''}`:'Quantité restante non communiquée'}{window.percent_remaining!==null?` · ${window.percent_remaining} % restant`:''}{window.overage_count!==null?` · Dépassement : ${window.overage_count}`:''}{window.overage_permitted===true?' · Dépassement autorisé':''}</p>):<p>GitHub n’a communiqué aucun quota pour ce compte.</p>}</div>}</div>}
  {googleAccounts.length>0&&<div className="gateway-panel"><h2>Compte Antigravity</h2><p>Démarre la passerelle ou ajoute un compte Google enregistré. Prise en charge expérimentale : conversations texte Chat, Responses ou Messages. Les outils simples sont expérimentaux en Chat, Responses et Messages ; Chat et Responses diffusent le texte progressivement sans outils ; avec outils, le flux attend la réponse complète, Messages attend la réponse complète. Le lancement de Codex et Claude Code est expérimental et reste à vérifier avec un compte réel ; le réglage de raisonnement de Claude Code est actuellement limité à Gemini 3.</p><div className="gateway-form"><label>Compte<select value={googleAccount} disabled={busy} onChange={e=>setGoogleAccount(e.target.value)}>{googleAccounts.map(account=><option key={account.id} value={account.id}>{account.email}</option>)}</select></label><label>Identifiant de routage<input value={googleId} onChange={e=>setGoogleId(e.target.value)} pattern="[a-z0-9_-]{1,40}" disabled={busy}/></label><button type="button" className="button ghost" disabled={busy||loadingModels||!googleAccount||!/^[a-z0-9_-]{1,40}$/.test(googleId)} onClick={()=>void addAntigravity()}>{status?'Ajouter Antigravity':'Démarrer avec Antigravity'}</button></div></div>}
  {status&&<div className="gateway-panel"><h2>Fournisseurs actifs</h2><p>Utilise <code>identifiant/modèle</code> dans ton outil pour choisir le fournisseur. Un nom sans barre oblique utilise le fournisseur par défaut. Si le nom contient déjà une barre oblique, ajoute aussi le préfixe du fournisseur.</p><ul className="gateway-models">{providers.map(item=><li key={item.id}><code>{item.id}</code><span>{item.antigravity?'Antigravity · Chat texte expérimental':item.anthropic_native?'Anthropic natif':item.chat_only?'Chat · traduction':'Responses natif'}{item.is_default?' · Par défaut':''}</span>{!item.is_default&&<button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void removeProvider(item.id)}>Retirer</button>}</li>)}</ul>{providers.some(item=>item.anthropic_native)&&<p>Anthropic natif accepte Messages et traduit les requêtes Responses courantes ; certaines fonctions avancées restent indisponibles.</p>}</div>}
  {status&&providers.some(item=>item.base_url==='https://openrouter.ai/api/v1/'&&!item.anthropic_native)&&<div className="gateway-panel"><h2>Limites OpenRouter</h2><p>Consulte l’utilisation et le plafond de la clé active, en dollars. Le montant restant correspond à la limite de la clé, pas nécessairement au solde du compte.</p><ul className="gateway-models">{providers.filter(item=>item.base_url==='https://openrouter.ai/api/v1/'&&!item.anthropic_native).map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadRouterUsage(item.id)}>Lire les limites</button></li>)}</ul>{routerUsage&&<div role="status"><p><strong>{routerUsage.id}</strong> · Limite : {routerUsage.data.limit===null?'non communiquée':`${routerUsage.data.limit} $`} · Restant : {routerUsage.data.remaining===null?'non communiqué':`${routerUsage.data.remaining} $`}</p><p>Utilisation : {routerUsage.data.usage===null?'non communiquée':`${routerUsage.data.usage} $`} · Aujourd’hui : {routerUsage.data.daily===null?'—':`${routerUsage.data.daily} $`} · Cette semaine : {routerUsage.data.weekly===null?'—':`${routerUsage.data.weekly} $`} · Ce mois : {routerUsage.data.monthly===null?'—':`${routerUsage.data.monthly} $`}</p>{routerUsage.data.reset&&<p>Renouvellement du plafond : {({daily:'quotidien',weekly:'hebdomadaire',monthly:'mensuel'} as Record<string,string>)[routerUsage.data.reset]??routerUsage.data.reset}.</p>}</div>}</div>}
  {status&&providers.some(item=>['https://api.deepseek.com/','https://api.deepseek.com/v1/'].includes(item.base_url)&&!item.anthropic_native)&&<div className="gateway-panel"><h2>Solde DeepSeek API</h2><p>Lis le solde du compte associé à la clé API active.</p><ul className="gateway-models">{providers.filter(item=>['https://api.deepseek.com/','https://api.deepseek.com/v1/'].includes(item.base_url)&&!item.anthropic_native).map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadDeepseekBalance(item.id)}>Lire le solde</button></li>)}</ul>{deepseekBalance&&<div role="status"><p><strong>{deepseekBalance.id}</strong> · {deepseekBalance.data.available?'Solde indiqué comme disponible pour l’API':'Solde indiqué comme insuffisant pour l’API'}</p>{deepseekBalance.data.balances.length?deepseekBalance.data.balances.map(balance=><p key={balance.currency}>Solde total : {balance.total} {balance.currency} · Crédit offert : {balance.granted??'—'} · Crédit rechargé : {balance.topped_up??'—'}</p>):<p>Montants non communiqués.</p>}</div>}</div>}
  {status&&providers.some(item=>glmQuotaUrls.includes(item.base_url))&&<div className="gateway-panel"><h2>Quotas GLM Coding Plan</h2><p>Lis les limites communiquées pour la clé active ; les offres et les régions peuvent avoir des fenêtres différentes.</p><ul className="gateway-models">{providers.filter(item=>glmQuotaUrls.includes(item.base_url)).map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadGlmQuota(item.id)}>Lire les quotas</button></li>)}</ul>{glmQuota&&<div role="status"><p><strong>{glmQuota.id}</strong></p>{glmQuota.data.windows.length?glmQuota.data.windows.map((window,index)=><p key={`${window.kind}-${window.unit}-${index}`}><strong>{window.kind==='TIME_LIMIT'?'Outils MCP':window.unit===3?'Modèles · 5 heures':window.unit===6?'Modèles · semaine':`Modèles · période ${window.unit??'non précisée'}`}</strong>{window.percent_used!==null?` · ${window.percent_used} % utilisé`:''}{window.used!==null?` · Utilisé : ${window.used}`:''}{window.limit!==null?` · Limite : ${window.limit}`:''}{window.remaining!==null?` · Restant : ${window.remaining}`:''}{window.reset_ms!==null?` · Renouvellement : ${new Date(window.reset_ms).toLocaleString()}`:''}</p>):<p>Le fournisseur n’a communiqué aucun quota.</p>}</div>}</div>}
  {status&&providers.some(item=>item.base_url==='https://api.cline.bot/api/v1/')&&<div className="gateway-panel"><h2>Quotas ClinePass</h2><p>Consulte les fenêtres renvoyées pour la clé Cline active. Leur disponibilité dépend de ton abonnement.</p><ul className="gateway-models">{providers.filter(item=>item.base_url==='https://api.cline.bot/api/v1/').map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadClineUsage(item.id)}>Lire les quotas</button></li>)}</ul>{clineUsage&&<div role="status"><p><strong>{clineUsage.id}</strong></p>{clineUsage.data.windows.length?clineUsage.data.windows.map(window=><p key={window.kind}><strong>{({five_hour:'5 heures',weekly:'Semaine',monthly:'Mois'} as Record<string,string>)[window.kind]??window.kind}</strong> · {window.percent_used} % utilisé{window.reset?` · Renouvellement : ${formatClineReset(window.reset)}`:''}</p>):<p>Cline n’a communiqué aucune fenêtre de quota.</p>}</div>}</div>}
  {status&&providers.some(item=>item.base_url==='https://opencode.ai/zen/go/v1/')&&<div className="gateway-panel"><h2>Quotas OpenCode Go</h2><p>Lis les pourcentages d’utilisation de l’abonnement associé à la clé active.</p><ul className="gateway-models">{providers.filter(item=>item.base_url==='https://opencode.ai/zen/go/v1/').map(item=><li key={item.id}><code>{item.id}</code><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void loadOpenCodeUsage(item.id)}>Lire les quotas</button></li>)}</ul>{openCodeUsage&&<div role="status"><p><strong>{openCodeUsage.id}</strong></p>{openCodeUsage.data.windows.length?openCodeUsage.data.windows.map(window=><p key={window.kind}><strong>{({rolling:'5 heures',weekly:'Semaine',monthly:'Mois'} as Record<string,string>)[window.kind]??window.kind}</strong> · {window.percent_used} % utilisé{window.status==='rate-limited'?' · Limite atteinte':''} · Renouvellement : {window.reset}</p>):<p>OpenCode n’a communiqué aucun quota.</p>}</div>}</div>}
  {status&&<div className="gateway-panel"><h2>Connecter un outil</h2><p>Renseigne cette adresse et cette clé dans un outil compatible OpenAI. La clé change à chaque démarrage.</p><div className="gateway-copy"><label htmlFor="gateway-address">Adresse locale</label><span><input id="gateway-address" readOnly value={status.address}/><button type="button" className="icon-button" onClick={()=>void copy(status.address)} aria-label="Copier l’adresse"><Copy size={16}/></button></span></div><div className="gateway-copy"><label htmlFor="gateway-claude-address">Adresse de base Claude Code</label><span><input id="gateway-claude-address" readOnly value={new URL(status.address).origin}/><button type="button" className="icon-button" onClick={()=>void copy(new URL(status.address).origin)} aria-label="Copier l’adresse Claude Code"><Copy size={16}/></button></span></div><div className="gateway-copy"><label htmlFor="gateway-token">Clé de la passerelle</label><span><input id="gateway-token" readOnly value={status.token} type={showToken?'text':'password'}/><button type="button" className="button ghost gateway-reveal" onClick={()=>setShowToken(value=>!value)}>{showToken?'Masquer':'Afficher'}</button><button type="button" className="icon-button" onClick={()=>void copy(status.token)} aria-label="Copier la clé"><Copy size={16}/></button></span></div><p>Pour Claude Code, utilise l’adresse de base Claude Code et la même clé. Les outils intégrés, le raisonnement étendu et le comptage de tokens ne sont pas encore tous pris en charge.</p><button type="button" className="button ghost" disabled={busy||loadingModels} onClick={()=>void stop()}>Arrêter la passerelle</button></div>}
  {status&&<div className="gateway-panel"><h2>Ouvrir un outil</h2><p>Lance Codex ou Claude Code dans une nouvelle fenêtre avec la passerelle active. Laisse Nivra ouverte pendant la session.</p><form className="gateway-form" onSubmit={launchCodex}><label>Modèle<input value={launchModel} onChange={e=>setLaunchModel(e.target.value)} placeholder="fournisseur/modèle" required maxLength={200}/></label><label>Dossier du projet <span>(Documents par défaut)</span><input value={workspace} onChange={e=>setWorkspace(e.target.value)} placeholder="C:\\…\\mon-projet"/></label><button type="submit" className="button primary" disabled={busy||loadingModels}>Ouvrir Codex</button><button type="button" className="button ghost" disabled={busy||loadingModels||!launchModel} onClick={()=>void launchClaude()}>Ouvrir Claude Code</button></form><p>Pour les fournisseurs traduits, certaines fonctions avancées sont indisponibles ; la recherche web intégrée de Codex est désactivée dans cette session.</p></div>}
  {status&&<div className="gateway-panel"><h2>Modèles du fournisseur</h2><p>Consulte les identifiants réellement renvoyés par le fournisseur pour les utiliser dans ton outil.</p><label className="gateway-provider-select">Fournisseur<select value={selectedProvider} disabled={loadingModels||busy} onChange={e=>{setSelectedProvider(e.target.value);setModels(null)}}>{providers.map(p=><option key={p.id} value={p.id}>{p.id}</option>)}</select></label><button type="button" className="button ghost" disabled={loadingModels||busy||!selectedProvider} onClick={()=>void loadModels()}>{loadingModels?'Lecture…':models?'Actualiser les modèles':'Charger les modèles'}</button>{models&&<>{models.length?<ul className="gateway-models">{models.map(model=><li key={model}><code>{selectedProvider}/{model}</code><button type="button" className="button ghost" onClick={()=>{setLaunchModel(`${selectedProvider}/${model}`);setMessage('Modèle sélectionné pour ton outil.')}}>Choisir</button><button type="button" className="icon-button" aria-label={`Copier ${selectedProvider}/${model}`} onClick={()=>void copy(`${selectedProvider}/${model}`)}><Copy size={15}/></button></li>)}</ul>:<p>Aucun modèle n’a été renvoyé.</p>}</>}</div>}
  {message&&<p className="gateway-message" role="status">{message}</p>}
 </section>
}
