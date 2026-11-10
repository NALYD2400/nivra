export type GatewayAccount={id:string;email:string;project:string;credential_ref:string;project_error?:string|null};

// Google's verified account id controls replacement; an email alone must never replace credentials.
export function upsertAntigravityAccount(accounts:GatewayAccount[],account:GatewayAccount){
 const previous=accounts.find(item=>item.id===account.id);
 const next=previous?accounts.map(item=>item.id===account.id?account:item):[...accounts,account];
 return {next,previous};
}
