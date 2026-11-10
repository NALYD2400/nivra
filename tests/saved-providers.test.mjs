import test from 'node:test';
import assert from 'node:assert/strict';
import { parseSavedProviders } from '../src/saved-providers.ts';

test('restores only safe non-secret provider settings',()=>{
 const ref='a'.repeat(32);
 const raw=JSON.stringify([{id:'openai',baseUrl:'https://api.example/v1',chatOnly:true,credentialRef:ref,key:'must-not-survive'},
  {id:'openai',baseUrl:'https://duplicate',chatOnly:false,credentialRef:null},
  {id:'../bad',baseUrl:'https://example',chatOnly:false,credentialRef:ref}]);
 assert.deepEqual(parseSavedProviders(raw),[{id:'openai',baseUrl:'https://api.example/v1',chatOnly:true,anthropicNative:false,credentialRef:ref}]);
 assert.deepEqual(parseSavedProviders('invalid'),[]);
});

test('restores native providers and rejects conflicting protocols',()=>{
 const input=[{id:'anthropic',baseUrl:'https://api.anthropic.com/v1',chatOnly:false,anthropicNative:true,credentialRef:null},
  {id:'invalid',baseUrl:'https://example.com',chatOnly:true,anthropicNative:true,credentialRef:null}];
 assert.deepEqual(parseSavedProviders(JSON.stringify(input)),[input[0]]);
});

test('restores Kimi account metadata only for the official endpoint',()=>{
 const entry={id:'kimi',baseUrl:'https://api.kimi.com/coding/v1',chatOnly:true,anthropicNative:false,credentialRef:'b'.repeat(32),authKind:'kimi'};
 assert.deepEqual(parseSavedProviders(JSON.stringify([entry])),[entry]);
 assert.deepEqual(parseSavedProviders(JSON.stringify([{...entry,baseUrl:'https://evil.example/v1'}])),[]);
});

test('restores Grok account metadata only for the subscription endpoint',()=>{
 const entry={id:'grok',baseUrl:'https://cli-chat-proxy.grok.com/v1',chatOnly:true,anthropicNative:false,credentialRef:'c'.repeat(32),authKind:'xai'};
 assert.deepEqual(parseSavedProviders(JSON.stringify([entry])),[entry]);
 for(const change of [{baseUrl:'https://api.x.ai/v1'},{baseUrl:'https://evil.example/v1'},{credentialRef:null},{chatOnly:false}])assert.deepEqual(parseSavedProviders(JSON.stringify([{...entry,...change}])),[]);
});

test('restores Copilot account metadata only for its provider endpoint',()=>{
 const entry={id:'copilot',baseUrl:'https://api.githubcopilot.com',chatOnly:true,anthropicNative:false,credentialRef:'d'.repeat(32),authKind:'copilot'};
 assert.deepEqual(parseSavedProviders(JSON.stringify([entry])),[entry]);
 for(const change of [{baseUrl:'https://evil.example'},{credentialRef:null},{chatOnly:false}])assert.deepEqual(parseSavedProviders(JSON.stringify([{...entry,...change}])),[]);
});

test('restores ChatGPT plan only for the OpenAI Responses endpoint',()=>{
 const entry={id:'chatgpt',baseUrl:'https://api.openai.com/v1',chatOnly:false,anthropicNative:false,credentialRef:'e'.repeat(32),authKind:'chatgpt'};
 assert.deepEqual(parseSavedProviders(JSON.stringify([entry])),[entry]);
 for(const change of [{baseUrl:'https://evil.example/v1'},{credentialRef:null},{chatOnly:true}])assert.deepEqual(parseSavedProviders(JSON.stringify([{...entry,...change}])),[]);
});

test('restores Qwen account only for its subscription endpoint',()=>{
 const entry={id:'qwen-account',baseUrl:'https://portal.qwen.ai/v1',chatOnly:true,anthropicNative:false,credentialRef:'f'.repeat(32),authKind:'qwen'};
 assert.deepEqual(parseSavedProviders(JSON.stringify([entry])),[entry]);
 for(const change of [{baseUrl:'https://evil.example/v1'},{credentialRef:null},{chatOnly:false}])assert.deepEqual(parseSavedProviders(JSON.stringify([{...entry,...change}])),[]);
});
test('restores Nous account only for its subscription endpoint',()=>{
 const entry={id:'nous',baseUrl:'https://inference-api.nousresearch.com/v1',chatOnly:true,anthropicNative:false,credentialRef:'a'.repeat(32),authKind:'nous'};
 assert.deepEqual(parseSavedProviders(JSON.stringify([entry])),[entry]);
 for(const change of [{baseUrl:'https://evil.example/v1'},{credentialRef:null},{chatOnly:false}])assert.deepEqual(parseSavedProviders(JSON.stringify([{...entry,...change}])),[]);
});
