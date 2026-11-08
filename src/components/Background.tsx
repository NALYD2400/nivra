import { useEffect, useState } from 'react';
import type { Appearance } from '../Appearance';
import RibbonGlow from './originkit/ui/ribbon-glow';

export function Background({appearance}:{appearance:Appearance}) {
  const [visible,setVisible]=useState(!document.hidden);
  useEffect(()=>{
    const updateVisibility=()=>setVisible(!document.hidden);
    document.addEventListener('visibilitychange',updateVisibility);
    return ()=>document.removeEventListener('visibilitychange',updateVisibility);
  },[]);
  if (!appearance.glow || !visible) return null;
  return <div className="ribbon-background" aria-hidden="true" style={{opacity:appearance.glowIntensity/100}}>
    <RibbonGlow background={appearance.theme==='dark'?'#111115':'#f5f4f8'} color1="#A67CFF" color2="#4B1CBF" speed={50} hover={100} style={{position:'absolute',inset:0,minWidth:0,minHeight:0}}/>
  </div>;
}
