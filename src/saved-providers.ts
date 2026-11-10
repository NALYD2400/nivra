export type SavedProvider = {id:string; baseUrl:string; chatOnly:boolean; anthropicNative:boolean; credentialRef:string|null; authKind?:'kimi'|'xai'|'copilot'|'chatgpt'|'qwen'|'nous'|'kiro'};
export const savedProvidersKey='nivra-gateway-providers-v1';

export function parseSavedProviders(raw:string|null):SavedProvider[] {
 try {
  const value:unknown=JSON.parse(raw||'[]');
  if(!Array.isArray(value))return [];
  const seen=new Set<string>();
  return value.filter((item):item is SavedProvider=>{
   if(!item||typeof item!=='object')return false;
   const candidate=item as Record<string,unknown>;
   if(typeof candidate.id!=='string'||!/^[a-z0-9_-]{1,40}$/.test(candidate.id)||seen.has(candidate.id)||
      typeof candidate.baseUrl!=='string'||candidate.baseUrl.length>2000||typeof candidate.chatOnly!=='boolean'||
      (candidate.anthropicNative!==undefined&&typeof candidate.anthropicNative!=='boolean')||
      (candidate.anthropicNative===true&&candidate.chatOnly===true)||
      (candidate.authKind!==undefined&&candidate.authKind!=='kimi'&&candidate.authKind!=='xai'&&candidate.authKind!=='copilot'&&candidate.authKind!=='chatgpt'&&candidate.authKind!=='qwen'&&candidate.authKind!=='nous'&&candidate.authKind!=='kiro')||
      (candidate.authKind==='kimi'&&(candidate.baseUrl!=='https://api.kimi.com/coding/v1'||candidate.chatOnly!==true||candidate.anthropicNative===true||candidate.credentialRef===null))||
      (candidate.authKind==='xai'&&(candidate.baseUrl!=='https://cli-chat-proxy.grok.com/v1'||candidate.chatOnly!==true||candidate.anthropicNative===true||candidate.credentialRef===null))||
      (candidate.authKind==='copilot'&&(candidate.baseUrl!=='https://api.githubcopilot.com'||candidate.chatOnly!==true||candidate.anthropicNative===true||candidate.credentialRef===null))||
      (candidate.authKind==='nous'&&(candidate.baseUrl!=='https://inference-api.nousresearch.com/v1'||candidate.chatOnly!==true||candidate.anthropicNative===true||candidate.credentialRef===null))||
      (candidate.authKind==='kiro'&&(candidate.baseUrl!=='https://codewhisperer.us-east-1.amazonaws.com'||candidate.chatOnly!==true||candidate.anthropicNative===true||candidate.credentialRef===null))||
      (candidate.authKind==='qwen'&&(candidate.baseUrl!=='https://portal.qwen.ai/v1'||candidate.chatOnly!==true||candidate.anthropicNative===true||candidate.credentialRef===null))||
      (candidate.authKind==='chatgpt'&&(candidate.baseUrl!=='https://api.openai.com/v1'||candidate.chatOnly!==false||candidate.anthropicNative===true||candidate.credentialRef===null))||
      !(candidate.credentialRef===null||(typeof candidate.credentialRef==='string'&&/^[a-f0-9]{32}$/.test(candidate.credentialRef))))return false;
   seen.add(candidate.id);return true;
  }).map(({id,baseUrl,chatOnly,anthropicNative,credentialRef,authKind})=>({id,baseUrl,chatOnly,anthropicNative:anthropicNative??false,credentialRef,...(authKind?{authKind}:{})}));
 } catch { return []; }
}
