# Nivra

Application Windows pour suivre ses comptes et les quotas disponibles. Elle utilise React, Vite et Tauri 2.

## Installation

Télécharger l’installateur Windows depuis les releases GitHub et l’exécuter. Nivra vérifie ensuite les nouvelles versions au lancement et régulièrement pendant son utilisation. La mise à jour peut aussi être recherchée dans **Réglages → Mises à jour** ; son installation est déclenchée par l’utilisateur.

Les mises à jour Tauri sont signées et vérifiées avant installation. L’installateur n’est pas signé avec un certificat de signature Windows ; Windows peut afficher un avertissement SmartScreen.

## Développement

Installer Node.js, Rust et les prérequis Windows de Tauri, puis lancer `npm ci` et `npm run tauri -- dev`. Pour un installateur, configurer `TAURI_SIGNING_PRIVATE_KEY` et `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, puis lancer `npm run tauri -- build`.

## Comptes et limites

- **Codex** : plusieurs comptes peuvent être connectés avec des profils séparés. Les quotas fournis par Codex sont actualisés périodiquement.
- **Claude Code** : une connexion via un profil séparé est proposée ; l’affichage des quotas n’est pas encore disponible.
- **Ordre des comptes** : les cartes Codex, Claude et Cursor/Grok de « Mes comptes » se déplacent avec la même poignée, à la souris ou avec les flèches haut/bas. L’ordre est conservé après redémarrage, sans ajouter de faux profil Cursor aux comptes Codex.
- **Services** : une page rassemble les fournisseurs sous forme de cartes, avec comptes ajoutés, abonnement communiqué et aperçu du quota. Le menu principal reste limité à Vue d’ensemble, Prévisions, Services et Réglages. Les pages de détail des fournisseurs sont accessibles depuis leurs cartes, puis un bouton Services permet de revenir à la liste. Cursor et Grok sont signalés comme liés lorsque la réserve est unifiée.

- **Cursor et xAI / Grok** : clique sur **Connecter mon compte**. Une fenêtre de connexion officielle s’ouvre dans Nivra, sans lancer l’éditeur Cursor. Après identification, Nivra vérifie le compte et lit le quota automatiquement. La session web reste dans un profil WebView2 dédié, protégé par Windows ; aucun cookie n’est transmis à l’interface ni stocké dans les données JSON de Nivra. Les cookies de connexion xAI/Google ne sont jamais envoyés aux API Cursor. Fermer ou annuler la connexion interrompt son suivi. Les données sont actualisées toutes les cinq minutes lorsque le suivi est activé. **Arrêter le suivi** désactive ces lectures.
- Si Cursor renvoie explicitement `isUnified: true`, Nivra affiche une seule réserve commune Cursor / Grok / Grok Bot et masque les anciens compteurs mensuels. Les deux pages présentent la même réserve et le résumé ne compte le compte qu’une fois. Si le service ne confirme pas l’unification, Nivra n’invente pas de réserve partagée ; les quotas renvoyés restent distincts et les données absentes sont indiquées comme indisponibles.
- Le nom de l’abonnement est affiché lorsqu’il est communiqué par le service. Pour un compte unifié, le nom du forfait actuel (`cursorPlanName`) est prioritaire sur le libellé générique « Grok Bot Plan » et l’ancien abonnement Cursor. Aucun forfait n’est déduit du prix ou du quota.
- **Prévisions** : les comptes Codex, Claude et Cursor connectés apparaissent automatiquement dans le sélecteur. Chaque nouvelle lecture de quota alimente l’historique local. La réserve hebdomadaire Cursor/Grok est suivie une seule fois ; la comparaison regroupe les périodes de même durée, en séparant les sous-quotas Cursor. Les limites absentes ne créent pas de faux graphiques. Le rythme conseillé est disponible dès le premier relevé ; une projection de consommation demande au moins quinze minutes d’historique. Aucun historique antérieur à la connexion n’est inventé.
- Les lectures utilisent des interfaces de tableau de bord susceptibles de changer. Elles sont testées avec des réponses simulées ; une lecture réelle doit être validée avec le compte de l’utilisateur. Les crédits supplémentaires et les modèles tiers ne sont pas additionnés au quota inclus. Ce suivi ne mesure pas un solde de facturation API xAI indépendant.
- **Antigravity** reste retiré. Les anciens profils isolés Cursor/Antigravity ne sont pas recréés ; le suivi utilise uniquement le profil web de connexion de Nivra, sans lire la session de l’éditeur ou les profils des navigateurs existants.

Les données affichées dépendent des services et de leurs interfaces disponibles. La connexion aux comptes se fait par les outils des fournisseurs ; Nivra ne demande pas les mots de passe.

## Publier une version

La version de `src-tauri/tauri.conf.json` doit correspondre aux versions de `package.json` et `src-tauri/Cargo.toml`. Après avoir mis à jour ces fichiers et `package-lock.json` / `Cargo.lock`, pousser le code, puis un tag `v` suivi de cette version (par exemple `v0.1.1`). Le workflow GitHub construit l’installateur, sa signature et le fichier `latest.json` pour la mise à jour.

Le dépôt doit disposer des secrets GitHub Actions `TAURI_SIGNING_PRIVATE_KEY` et `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Conserver une sauvegarde privée de la clé de signature : la perdre empêchera les installations existantes d’accepter les futures mises à jour. Ne jamais la publier dans le dépôt.
