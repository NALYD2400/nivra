export function expiresSoon(expiresAt:number|null|undefined,now:number):boolean {
 return typeof expiresAt==='number'&&Number.isFinite(expiresAt)&&expiresAt*1000>now&&expiresAt*1000-now<=3*24*60*60*1000;
}
