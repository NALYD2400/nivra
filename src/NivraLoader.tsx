import { useId } from 'react';
import { RefreshCw } from 'lucide-react';
import './nivra-loader.css';

type Props = { label?: string; compact?: boolean };

export function NivraLoader({label,compact=false}:Props){
 const id=useId();
 return <span className={`nivra-loader${compact?' nivra-loader--compact':''}`} role={label?'status':undefined} aria-labelledby={label?id:undefined} aria-hidden={label?undefined:true}>
  <span className="nivra-loader-mark" aria-hidden="true"><RefreshCw size={16} strokeWidth={1.6}/></span>
  {label&&<span id={id} className="nivra-loader-label">{label}<span className="nivra-loader-dots" aria-hidden="true"><i/><i/><i/></span></span>}
 </span>;
}
