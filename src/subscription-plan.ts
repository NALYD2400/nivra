export function subscriptionPlan(value:string|null|undefined){
 const plan=value?.trim();
 if(!plan)return 'Offre non communiquée';
 const names:Record<string,string>={free:'Gratuit',free_trial:'Essai gratuit',go:'Go',plus:'Plus',premium:'Premium',super:'Super',ultra:'Ultra',pro:'Pro',pro_plus:'Pro+', 'pro+':'Pro+',team:'Team',teams:'Teams',business:'Business',enterprise:'Enterprise',max:'Max'};
 return names[plan.toLowerCase()]??plan;
}
