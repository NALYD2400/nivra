import { useId } from 'react';

/** Small vector symbols drawn for connection messages, with no image backgrounds. */
export function ConnectionIcon({kind,size=18,className=''}:{kind:'info'|'loading'|'browser';size?:number;className?:string}){
 const id=useId();
 return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" className={`connection-symbol ${className}`} aria-hidden="true" focusable="false">
  <defs><linearGradient id={id} x1="4" y1="3" x2="20" y2="22" gradientUnits="userSpaceOnUse"><stop stopColor="var(--connection-icon-highlight)"/><stop offset="1" stopColor="var(--connection-icon-base)"/></linearGradient></defs>
  {kind==='info'?<><circle cx="12" cy="12" r="9" stroke={`url(#${id})`} strokeWidth="1.8"/><path d="M11 11h1v6m-1 0h2" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"/><circle cx="12" cy="7.5" r="1.15" fill="currentColor"/></>:kind==='loading'?<><circle cx="12" cy="12" r="8.5" stroke="currentColor" strokeWidth="2" opacity=".16"/><path d="M12 3.5a8.5 8.5 0 0 1 8.5 8.5" stroke={`url(#${id})`} strokeWidth="2.6" strokeLinecap="round"/><circle cx="12" cy="3.5" r="1.3" fill="currentColor"/></>:<><rect x="3" y="5" width="18" height="15" rx="3" stroke={`url(#${id})`} strokeWidth="1.7"/><path d="M3 9h18" stroke="currentColor" strokeOpacity=".45" strokeWidth="1.5"/><circle cx="6.3" cy="7" r=".7" fill="currentColor"/><circle cx="8.8" cy="7" r=".7" fill="currentColor"/><path d="m10 16 5-5m-4 0h4v4" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"/></>}
 </svg>;
}
